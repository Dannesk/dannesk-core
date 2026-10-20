use crate::decrypt::decrypt_data;
use crate::bridge::json_storage::read_json;
use dannesk_btc_codec::address::Address;
use dannesk_btc_codec::bip32::{DerivationPath, Xpriv};
use dannesk_btc_codec::bip39::Mnemonic;
use dannesk_btc_codec::secp256k1::SecretKey;
use serde::Deserialize;
use std::fmt;
use zeroize::{Zeroize, Zeroizing};

use crate::secure::{SecureBytes, SecureString};


pub struct BitcoinWallet {
    pub address: String,
    keys: std::collections::HashMap<String, Zeroizing<[u8; 32]>>,
}

impl fmt::Debug for BitcoinWallet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "BitcoinWallet({}, {} keys)", self.address, self.keys.len())
    }
}

impl BitcoinWallet {
    pub fn key_for(&self, address: &str) -> Option<SecretKey> {
        self.keys.get(address).and_then(|k| SecretKey::from_secret_bytes(**k).ok())
    }

   
    #[cfg(test)]
    pub(crate) fn for_test(address: String, keys: Vec<(String, [u8; 32])>) -> Self {
        Self {
            address,
            keys: keys.into_iter().map(|(a, k)| (a, Zeroizing::new(k))).collect(),
        }
    }
}

#[derive(Deserialize)]
struct EncryptedWalletData {
    encrypted_phrase: String,
    #[serde(default)]
    salt: String,
    iv: String,
}

pub fn authenticate_wallet(
    passphrase: Option<SecureString>,
    seed: Option<SecureString>,
    bip39: Option<SecureString>,
    wallet_address: &str,
) -> Result<BitcoinWallet, String> {

    let mnemonic_text: SecureString = match (passphrase, seed) {
        (None, Some(s)) => s,
        (Some(p), None) => {
            let stored_data: EncryptedWalletData = read_json("btc_encrypt.json")
                .map_err(|e| format!("Error: Encrypted wallet file not found or corrupted: {}", e))?;

            decrypt_data(
                p.as_str(),
                &stored_data.encrypted_phrase,
                &stored_data.salt,
                &stored_data.iv,
            )
            .map_err(|e| format!("Error: Decryption failed: {}", e))?
        }
        _ => return Err("Error: Must provide exactly one of passphrase or seed".to_string()),
    };

    let mnemonic = Mnemonic::parse(mnemonic_text.as_str())
        .map_err(|e| format!("Error: Invalid mnemonic: {}", e))?;

    let seed_passphrase = bip39.as_ref().map(|s| s.as_str()).unwrap_or("");
    let mut seed_arr = mnemonic.to_seed(seed_passphrase);
    let seed_bytes = SecureBytes::new(seed_arr.to_vec());
    seed_arr.zeroize();

    let xpriv = Xpriv::new_master(seed_bytes.as_bytes())
        .map_err(|e| format!("Error: Failed to create master key: {}", e))?;

    let derivation_path: DerivationPath = "m/84'/0'/0'/0/0".parse().unwrap();
    let child_xpriv = xpriv
        .derive(&derivation_path)
        .map_err(|e| format!("Error: Failed to derive private key: {}", e))?;

  
    let derived_address = Address::p2wpkh(&child_xpriv.public_key()).to_string();

 
    let script_type = crate::btc_script_type::stored();
    let (secret, derived_address) = match script_type {
        crate::btc_script_type::BtcScriptType::NativeSegwit => {
            (Zeroizing::new(child_xpriv.secret_key().to_secret_bytes()), derived_address)
        }
        other => {
            let child = xpriv
                .derive(&other.member_path(0, 0))
                .map_err(|e| format!("Error: Failed to derive private key: {}", e))?;
            let address = other.address(&child.public_key()).to_string();
            (Zeroizing::new(child.secret_key().to_secret_bytes()), address)
        }
    };
    if !wallet_address.is_empty() && derived_address != wallet_address {
        return Err(
            "Error: that credential opens a different wallet — check your 25th word".to_string(),
        );
    }

    let mut keys = std::collections::HashMap::new();
    keys.insert(derived_address.clone(), secret);

   
    for record in crate::wallet::btc_address_records() {
        if record.chain == 0 && record.index == 0 {
            continue; 
        }
        let member_path = script_type.member_path(record.chain, record.index);
        let member_xpriv = xpriv
            .derive(&member_path)
            .map_err(|e| format!("Error: member key derivation failed: {}", e))?;
        let member_address = script_type.address(&member_xpriv.public_key()).to_string();
        if member_address != record.address {
            return Err(format!(
                "Error: derived address for {}/{} does not match the stored wallet",
                record.chain, record.index
            ));
        }
        keys.insert(member_address, Zeroizing::new(member_xpriv.secret_key().to_secret_bytes()));
    }


    let stored: Result<serde_json::Value, _> = read_json("btc.json");
    if let Ok(stored) = stored
        && stored.get("account_xpub").and_then(|v| v.as_str()).is_none_or(|s| s.is_empty())
    {
        let account_xpub = xpriv
            .derive(&script_type.account_path())
            .map_err(|e| format!("Error: account derivation failed: {}", e))?
            .to_xpub()
            .to_string();
        let _ = crate::bridge::json_storage::update_json("btc.json", |data: &mut serde_json::Value| {
            if let Some(obj) = data.as_object_mut() {
                obj.insert("account_xpub".to_string(), serde_json::Value::String(account_xpub.clone()));
            }
        });
    }

    Ok(BitcoinWallet {
      
        address: derived_address,
        keys,
    })
}
