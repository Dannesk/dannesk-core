use crate::btc_script_type::BtcScriptType;
use crate::channel::{CHANNEL, ActivityLogState, PendingWallet, WSCommand};
use crate::wallet::ImportMode;
use crate::encrypt::encrypt_data;
use dannesk_btc_codec::address::Address;
use dannesk_btc_codec::bip32::{DerivationPath, Xpriv};
use dannesk_btc_codec::bip39::Mnemonic;
use std::str::FromStr;
use tokio::sync::mpsc::Sender;
use zeroize::Zeroize;

use crate::secure::{SecureBytes, SecureString};


struct Prepared {
    address: String,
    encrypted_phrase: String,
    salt: String,
    iv: String,
    method: String,
    script_type: BtcScriptType,
}

pub struct BTCImportLogic;

impl BTCImportLogic {
    pub async fn process(
        mnemonic_phrase: SecureString,
        bip39_pass: SecureString,
        encryption_pass: SecureString,
        mode: ImportMode,
        script_type: BtcScriptType,
        ws_tx: Sender<WSCommand>,
    ) -> Result<(), String> {
        let mut log = ActivityLogState::new(
            "Import Bitcoin wallet",
            &[
                ("derive", "Deriving keys"),
                ("connect", "Subscribing wallet to network"),
                ("verify", "Awaiting response"),
                ("save", "Import successful"),
            ],
        );
        log.start("derive");
        let _ = CHANNEL.activity_tx.send(Some(log.clone()));

        let crypto_result = tokio::task::spawn_blocking(
            move || -> Result<Prepared, String> {
                let mnemonic = Mnemonic::parse(mnemonic_phrase.as_str())
                    .map_err(|e| format!("Invalid mnemonic: {}", e))?;

                let mut seed_arr = mnemonic.to_seed(bip39_pass.as_str());
                let seed = SecureBytes::new(seed_arr.to_vec());
                seed_arr.zeroize();

                let xpriv = Xpriv::new_master(seed.as_bytes())
                    .map_err(|e| format!("Failed to create master key: {}", e))?;

                let derivation_path = DerivationPath::from_str("m/84'/0'/0'/0/0")
                    .map_err(|_| "Invalid derivation path".to_string())?;

                let child_xpriv = xpriv
                    .derive(&derivation_path)
                    .map_err(|e| format!("Derivation failed: {}", e))?;

                let address = Address::p2wpkh(&child_xpriv.public_key());

                let address = match script_type {
                    BtcScriptType::NativeSegwit => address,
                    other => {
                        let path = DerivationPath::from_str(other.path())
                            .map_err(|_| "Invalid derivation path".to_string())?;
                        let child = xpriv
                            .derive(&path)
                            .map_err(|e| format!("Derivation failed: {}", e))?;
                        other.address(&child.public_key())
                    }
                };

                let (encrypted_phrase, salt, iv, method) = match mode {
                    ImportMode::Standard => {
                        let (enc, salt, iv) = encrypt_data(encryption_pass.as_str(), mnemonic_phrase.as_str())
                            .map_err(|e| format!("Encryption failed: {}", e))?;
                        (enc, salt, iv, "standard".to_string())
                    }
                    ImportMode::Cold => {
                        (String::new(), String::new(), String::new(), "cold".to_string())
                    }
                };

                Ok(Prepared { address: address.to_string(), encrypted_phrase, salt, iv, method, script_type })
            },
        )
        .await;

        match crypto_result {
            Ok(Ok(prepared)) => {
                log.finish("derive");
                log.start("connect");
                let _ = CHANNEL.activity_tx.send(Some(log.clone()));

                let _ = ws_tx.try_send(WSCommand {
                    command: "import_bitcoin_wallet".to_string(),
                    wallet: Some(prepared.address.clone()),
                    pending: Some(PendingWallet {
                        address: prepared.address,
                        encrypted_phrase: prepared.encrypted_phrase,
                        salt: prepared.salt,
                        iv: prepared.iv,
                        method: prepared.method,
                        script_type: prepared.script_type,
                    }),
                    ..Default::default()
                });

                Ok(())
            }
            Ok(Err(e)) => {
                log.fail("derive", e.clone());
                let _ = CHANNEL.activity_tx.send(Some(log));
                Err(e)
            }
            _ => {
                let msg = "Internal thread error".to_string();
                log.fail("derive", msg.clone());
                let _ = CHANNEL.activity_tx.send(Some(log));
                Err(msg)
            }
        }
    }
}
