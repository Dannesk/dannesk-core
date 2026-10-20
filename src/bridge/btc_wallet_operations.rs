// src/bridge/btc_wallet_operations.rs

use crate::channel::{BtcTransactionState, CHANNEL, ActivityLogState, WSCommand};
use crate::bridge::json_storage::{self, get_config_path, remove_json, write_json};
use crate::encrypt::encrypt_data;
use dannesk_btc_codec::address::Address;
use dannesk_btc_codec::bip32::{DerivationPath, Xpriv};
use dannesk_btc_codec::bip39::Mnemonic;
use serde::Serialize;
use std::str::FromStr;
use tokio::sync::mpsc::Sender;
use zeroize::Zeroize;

use crate::secure::SecureString;

pub struct BitcoinWalletOperations;

impl BitcoinWalletOperations {

    pub async fn delete_key(wallet_address: String) {
        if let Ok(path) = get_config_path("btc_encrypt.json") {
            if path.exists() {
                let _ = remove_json("btc_encrypt.json");
            }
        }

        let _ = json_storage::update_json("btc.json", |data: &mut serde_json::Value| {
            if let Some(obj) = data.as_object_mut() {
                obj.insert("private_key_deleted".to_string(), serde_json::Value::Bool(true));
            }
        });

        let (current_balance, _, _, current_mode) = *CHANNEL.bitcoin_wallet_rx.borrow();
        let _ = CHANNEL.bitcoin_wallet_tx.send((current_balance, Some(wallet_address), true, current_mode));
    }

  
    pub async fn reimport_key(
        mnemonic_phrase: SecureString,
        bip39_pass: SecureString,
        encryption_pass: SecureString,
    ) -> Result<(), String> {
        let (_, wallet_address, _, _) = CHANNEL.bitcoin_wallet_rx.borrow().clone();
        let Some(expected_address) = wallet_address else {
            return Err("ERR: NO_WALLET_FOUND".to_string());
        };

        
        let expected = expected_address.clone();
        let script_type = crate::btc_script_type::stored();
        let result = tokio::task::spawn_blocking(
            move || -> Result<(String, String, String, String, String), String> {
                let mnemonic = Mnemonic::parse(mnemonic_phrase.as_str())
                    .map_err(|e| format!("Invalid recovery phrase: {}", e))?;

                let mut seed = mnemonic.to_seed(bip39_pass.as_str());

                let xpriv = Xpriv::new_master(&seed)
                    .map_err(|e| { seed.zeroize(); format!("Key derivation failed: {}", e) })?;
                seed.zeroize();

                let path = DerivationPath::from_str("m/84'/0'/0'/0/0")
                    .map_err(|_| "Invalid derivation path".to_string())?;
                let child = xpriv.derive(&path)
                    .map_err(|e| format!("Derivation failed: {}", e))?;

                let derived_address = Address::p2wpkh(&child.public_key()).to_string();

            
                let derived_address = match script_type {
                    crate::btc_script_type::BtcScriptType::NativeSegwit => derived_address,
                    other => {
                        let child = xpriv
                            .derive(&other.member_path(0, 0))
                            .map_err(|e| format!("Derivation failed: {}", e))?;
                        other.address(&child.public_key()).to_string()
                    }
                };

                if derived_address != expected {
                    return Err("ERR: PHRASE_ADDRESS_MISMATCH — this phrase does not match the stored wallet.".to_string());
                }

           
                let account_xpub = xpriv
                    .derive(&script_type.account_path())
                    .map_err(|e| format!("Account derivation failed: {}", e))?
                    .to_xpub()
                    .to_string();

                let (enc, salt, iv) = encrypt_data(encryption_pass.as_str(), mnemonic_phrase.as_str())
                    .map_err(|e| format!("Encryption failed: {}", e))?;
                Ok((enc, salt, iv, "standard".to_string(), account_xpub))
            },
        ).await;

        let (enc, salt, iv, method, account_xpub) = match result {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => return Err(e),
            Err(_) => return Err("Internal thread error".to_string()),
        };

        #[derive(Serialize)]
        struct EncryptedWalletData {
            encrypted_phrase: String,
            salt: String,
            iv: String,
        }

        if let Err(e) = write_json("btc_encrypt.json", &EncryptedWalletData {
            encrypted_phrase: enc,
            salt,
            iv,
        }) {
            return Err(format!("FS Error: {}", e));
        }

      
        let _ = json_storage::update_json("btc.json", |data: &mut serde_json::Value| {
            if let Some(obj) = data.as_object_mut() {
                obj.insert("private_key_deleted".to_string(), serde_json::Value::Bool(false));
                obj.insert("method".to_string(), serde_json::Value::String(method.clone()));
                if obj.get("account_xpub").and_then(|v| v.as_str()).is_none_or(|s| s.is_empty()) {
                    obj.insert("account_xpub".to_string(), serde_json::Value::String(account_xpub.clone()));
                }
            }
        });

        let (current_balance, _, _, _) = *CHANNEL.bitcoin_wallet_rx.borrow();
        let _ = CHANNEL.bitcoin_wallet_tx.send((current_balance, Some(expected_address), false, crate::channel::KeyMode::from_method(&method)));

        Ok(())
    }

    pub async fn remove_wallet(wallet_address: String, ws_tx: Sender<WSCommand>) {
        let mut log = ActivityLogState::new(
            "Remove Bitcoin wallet",
            &[
                ("keys",   "Remove keys"),
                ("wallet", "Remove wallet"),
                ("notify", "Unsubscribing wallet"),
            ],
        );
        log.start("keys");
        let _ = CHANNEL.activity_tx.send(Some(log.clone()));

        let _ = remove_json("btc_encrypt.json");

        log.finish("keys");
        log.start("wallet");
        let _ = CHANNEL.activity_tx.send(Some(log.clone()));

        if let Ok(path) = get_config_path("btc.json")
            && path.exists()
            && let Err(e) = remove_json("btc.json")
        {
            log.fail("wallet", format!("Error removing wallet file: {}", e));
            let _ = CHANNEL.activity_tx.send(Some(log));
            return;
        }

        log.finish("wallet");
        log.start("notify");
        let _ = CHANNEL.activity_tx.send(Some(log.clone()));

        let _ = ws_tx.try_send(WSCommand {
            command: "delete_bitcoin_wallet".to_string(),
            wallet: Some(wallet_address.clone()),
            ..Default::default()
        });

        log.finish("notify");
        let _ = CHANNEL.activity_tx.send(Some(log.clone()));

        let _ = CHANNEL.btc_transactions_tx.send(BtcTransactionState::default());
        let _ = CHANNEL.btc_utxos_tx.send((None, Vec::new()));
        let _ = CHANNEL.bitcoin_wallet_tx.send((0.0, None, false, crate::channel::KeyMode::Standard));
      
    }
}
