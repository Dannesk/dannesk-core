

use crate::bridge::json_storage;
use sha2::{Digest, Sha256};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use rand::RngExt;
use serde::{Deserialize, Serialize};

const FILE: &str = "gate.json";


pub const PIN_DIGITS: usize = 6;


pub const MAX_FAILS: u32 = 5;

#[derive(Serialize, Deserialize)]
struct GateData {
    hash: String, 
    salt: String, 
   
    #[serde(default)]
    fails: u32,
}

fn digest_str(secret: &str, salt: &[u8]) -> Vec<u8> {
    let mut h = Sha256::new();
    h.update(salt);
    h.update(secret.as_bytes());
    h.finalize().to_vec()
}

fn read() -> Option<GateData> {
    let data = json_storage::read_json::<GateData>(FILE).ok()?;
    (!data.hash.is_empty() && !data.salt.is_empty()).then_some(data)
}

pub fn exists() -> bool {
    read().is_some()
}

pub fn set_pin(pin: &str) -> Result<(), String> {
    let salt: [u8; 16] = rand::rng().random();
    let data = GateData {
        hash: BASE64.encode(digest_str(pin, &salt)),
        salt: BASE64.encode(salt),
        fails: 0,
    };
    json_storage::write_json(FILE, &data).map_err(|e| e.to_string())
}

pub fn verify_pin(pin: &str) -> Result<bool, String> {
    let data = read().ok_or("Could not read the gate record.")?;
    let (Ok(stored), Ok(salt)) = (BASE64.decode(&data.hash), BASE64.decode(&data.salt)) else {
        return Err("The gate record is corrupt (its hash could not be decoded).".into());
    };
    Ok(digest_str(pin, &salt).as_slice() == stored.as_slice())
}


pub fn register_fail() -> u32 {
    let Some(mut data) = read() else {
      
        return MAX_FAILS;
    };
    data.fails = (data.fails + 1).min(MAX_FAILS);
    let _ = json_storage::write_json(FILE, &data);
    MAX_FAILS.saturating_sub(data.fails)
}


pub fn delete() {
    let _ = json_storage::remove_json(FILE);
}

pub fn enabled() -> bool {
    json_storage::read_json::<serde_json::Value>("settings.json")
        .ok()
        .and_then(|j| j.get("gate_enabled").and_then(|v| v.as_bool()))
        .unwrap_or(true)
}

pub fn clear_attempts() {
    let Some(mut data) = read() else { return };
    if data.fails == 0 {
        return; 
    }
    data.fails = 0;
    let _ = json_storage::write_json(FILE, &data);
}
