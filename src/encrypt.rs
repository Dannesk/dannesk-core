use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, KeyInit},
};
use argon2::{Algorithm, Argon2, Params, Version};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use rand::RngExt;
use zeroize::Zeroize;

use crate::secure::SecureBytes;


pub fn encrypt_data(
    passphrase: &str,
    seed: &str,
) -> Result<(String, String, String), String> {
    let salt: [u8; 16] = rand::rng().random();
    let iv: [u8; 12] = rand::rng().random();

    match derive_key_from_passphrase(passphrase, &salt) {
        Ok(key) => {
            let cipher = Aes256Gcm::new_from_slice(key.as_bytes()).map_err(|e| e.to_string())?;

            let encrypted_data = cipher
                .encrypt(&Nonce::from(iv), seed.as_bytes())
                .map_err(|e| e.to_string())?;

            let base64_encrypted = BASE64.encode(&encrypted_data);
            let base64_salt = BASE64.encode(salt);
            let base64_iv = BASE64.encode(iv);

     
            Ok((base64_encrypted, base64_salt, base64_iv))
        }
        Err(e) => Err(e),
    }
}

fn derive_key_from_passphrase(passphrase: &str, salt: &[u8]) -> Result<SecureBytes, String> {
    let mut key = [0u8; 32];

    let params = Params::new(65536, 3, 4, None).map_err(|e| e.to_string())?;
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);

    argon2
        .hash_password_into(passphrase.as_bytes(), salt, &mut key)
        .map_err(|e| e.to_string())?;

    let result = SecureBytes::new(key.to_vec());
    key.zeroize();

    Ok(result)
}

