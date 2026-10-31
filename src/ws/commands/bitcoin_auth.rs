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
    secret: Zeroizing<[u8; 32]>,
}

impl fmt::Debug for BitcoinWallet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "BitcoinWallet({})", self.address)
    }
}

impl BitcoinWallet {
    pub fn secret_key(&self) -> Option<SecretKey> {
        SecretKey::from_secret_bytes(*self.secret).ok()
    }

    #[cfg(test)]
    pub(crate) fn for_test(address: String, secret: [u8; 32]) -> Self {
        Self { address, secret: Zeroizing::new(secret) }
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
            let path: DerivationPath = other
                .path()
                .parse()
                .map_err(|_| "Error: Invalid derivation path".to_string())?;
            let child = xpriv
                .derive(&path)
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

    Ok(BitcoinWallet {
        address: derived_address,
        secret,
    })
}
