use crate::decrypt::decrypt_data;
use crate::bridge::json_storage::read_json;
use dannesk_btc_codec::bip32::{DerivationPath, Xpriv};
use dannesk_btc_codec::bip39::Mnemonic;
use dannesk_btc_codec::secp256k1::{PublicKey, SecretKey};
use ripemd::Ripemd160;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use zeroize::Zeroize;

use crate::secure::{SecureBytes, SecureString};

#[derive(Deserialize)]
struct EncryptedWalletData {
    encrypted_phrase: String,
    #[serde(default)]
    salt: String,
    iv: String,
}

pub struct Bip44Wallet {
    pub address: String,
    pub secret_key: SecretKey,
    pub public_key: PublicKey,
}

// The private key derived from the seed is erased when the wallet drops.
impl Drop for Bip44Wallet {
    fn drop(&mut self) {
        self.secret_key.non_secure_erase();
    }
}

pub fn authenticate_wallet(
    passphrase: Option<SecureString>,
    seed: Option<SecureString>,
    bip39: Option<SecureString>,
    wallet_address: &str,
) -> Result<Bip44Wallet, String> {

    let mnemonic_text: SecureString = match (passphrase, seed) {
        (None, Some(s)) => s,
        (Some(p), None) => {
            let stored_data: EncryptedWalletData = read_json("xrp_encrypt.json")
                .map_err(|e| format!("Error: XRP credentials not found: {}", e))?;

            decrypt_data(
                p.as_str(),
                &stored_data.encrypted_phrase,
                &stored_data.salt,
                &stored_data.iv,
            )
            .map_err(|_| "Error: Decryption failed".to_string())?
        }
        _ => return Err("Error: Must provide exactly one of passphrase or seed".to_string()),
    };

    let mnemonic = Mnemonic::parse(mnemonic_text.as_str())
        .map_err(|_| "Error: Invalid mnemonic".to_string())?;

    let seed_passphrase = bip39.as_ref().map(|s| s.as_str()).unwrap_or("");
    let mut seed_arr = mnemonic.to_seed(seed_passphrase);
    // Hold the raw seed in mlocked memory; wipe the transient stack array.
    let bip39_seed = SecureBytes::new(seed_arr.to_vec());
    seed_arr.zeroize();

    // `bip39 seed` is zeroized and unlocked when it drops at end of scope.
    let xpriv = Xpriv::new_master(bip39_seed.as_bytes())
        .map_err(|e| format!("Error: Failed to create master key: {}", e))?;

    let path: DerivationPath = "m/44'/144'/0'/0/0"
        .parse()
        .map_err(|_| "Error: Invalid derivation path".to_string())?;

    let child_xpriv = xpriv
        .derive(&path)
        .map_err(|e| format!("Error: Derivation failed: {}", e))?;

    let secret_key = *child_xpriv.secret_key();
    let public_key = child_xpriv.public_key();

   
    let account_id = {
        let rip_hash = Ripemd160::digest(Sha256::digest(public_key.serialize()));
        let mut id = [0u8; 21];
        id[0] = 0x00;
        id[1..].copy_from_slice(&rip_hash);
        id
    };
    let alphabet = bs58::Alphabet::new(b"rpshnaf39wBUDNEGHJKLM4PQRST7VWXYZ2bcdeCg65jkm8oFqi1tuvAxyz")
        .map_err(|_| "Error: Invalid address alphabet".to_string())?;
    let derived_address = bs58::encode(&account_id)
        .with_alphabet(&alphabet)
        .with_check()
        .into_string();
    if !wallet_address.is_empty() && derived_address != wallet_address {
        return Err(
            "Error: that credential opens a different wallet — check your 25th word".to_string(),
        );
    }

    Ok(Bip44Wallet {
        
        address: derived_address,
        secret_key,
        public_key,
    })
}
