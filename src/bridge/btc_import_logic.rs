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

/// What `process` prepares on the worker thread for `bitcoin_import_wallet` to
/// persist once the relay confirms. For Cold the encrypted fields are all empty.
struct Prepared {
    address: String,
    encrypted_phrase: String,
    salt: String,
    iv: String,
    method: String,
    /// The neutered account key (`m/{purpose}'/0'/0'` xpub) — what lets the
    /// receive screen derive fresh addresses WITHOUT the seed (rotation works
    /// on cold wallets too). Stored in btc.json; reveals addresses, never keys.
    account_xpub: String,
    /// The address type the user chose — persisted so every later derivation
    /// (members at signing, rotation) walks the same purpose and encoding.
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
                ("derive",  "Deriving keys"),
                ("connect", "Subscribing wallet to network"),
                ("verify",  "Awaiting response"),
                ("save",    "Import successful"),

            ],
        );
        log.start("derive");
        let _ = CHANNEL.activity_tx.send(Some(log.clone()));

        // Move the locked secrets straight into the worker thread — no clone,
        // no unlocked copy. They zeroize + unlock when the closure returns.
        let crypto_result = tokio::task::spawn_blocking(
            move || -> Result<Prepared, String> {
                let mnemonic = Mnemonic::parse(mnemonic_phrase.as_str())
                    .map_err(|e| format!("Invalid mnemonic: {}", e))?;

                let mut seed_arr = mnemonic.to_seed(bip39_pass.as_str());
                // Hold the raw seed in mlocked memory; wipe the transient stack array.
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

                // The other three address types (2026-09-14): #0 at THEIR
                // purpose and encoding. Native keeps the frozen block above
                // verbatim — this only replaces the answer for a wallet the
                // user said is taproot / nested / legacy.
                let address = match script_type {
                    BtcScriptType::NativeSegwit => address,
                    other => {
                        let child = xpriv
                            .derive(&other.member_path(0, 0))
                            .map_err(|e| format!("Derivation failed: {}", e))?;
                        other.address(&child.public_key())
                    }
                };

                // The account key — `m/{purpose}'/0'/0'`, neutered. It rides
                // the import (2026-09-20): the Bitcoin relay derives both
                // chains from it and walks them gap-20 itself, so this client
                // no longer derives an address window to send. A NEW
                // derivation site, self-contained on purpose (the #0 path
                // above is frozen and stays byte-for-byte as it was).
                let account_xpub = xpriv
                    .derive(&script_type.account_path())
                    .map_err(|e| format!("Account derivation failed: {}", e))?
                    .to_xpub()
                    .to_string();
                // Per-mode key protection. The address above is derived for all
                // three; only what we persist differs.
                let (encrypted_phrase, salt, iv, method) = match mode {
                    ImportMode::Standard => {
                        let (enc, salt, iv) = encrypt_data(encryption_pass.as_str(), mnemonic_phrase.as_str())
                            .map_err(|e| format!("Encryption failed: {}", e))?;
                        (enc, salt, iv, "standard".to_string())
                    }
                    ImportMode::Cold => {
                        // Watch-only — nothing about the key is persisted.
                        (String::new(), String::new(), String::new(), "cold".to_string())
                    }
                };

                Ok(Prepared { address: address.to_string(), encrypted_phrase, salt, iv, method, account_xpub, script_type })
            },
        )
        .await;

        match crypto_result {
            Ok(Ok(prepared)) => {
                log.finish("derive");
                log.start("connect");
                let _ = CHANNEL.activity_tx.send(Some(log.clone()));

                // The record rides the command; the socket task keeps it for
                // the reply (`RelayState::track`). The xpub and script type
                // ride again in the open, for the frame `execute` sends.
                let _ = ws_tx.try_send(WSCommand {
                    command: "import_bitcoin_wallet".to_string(),
                    wallet: Some(prepared.address.clone()),
                    xpub: Some(prepared.account_xpub.clone()),
                    script_type: Some(prepared.script_type.tag()),
                    pending: Some(PendingWallet {
                        address: prepared.address,
                        encrypted_phrase: prepared.encrypted_phrase,
                        salt: prepared.salt,
                        iv: prepared.iv,
                        method: prepared.method,
                        account_xpub: prepared.account_xpub,
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
