// ws/commands/bitcoin_import_wallet.rs

use crate::channel::{CHANNEL, PendingWallet, WSCommand};
use crate::bridge::json_storage::{read_bytes, read_json, remove_json, write_bytes, write_json};
use crate::ws::CRYPTO_OUTGOING_TX;
use serde::Serialize;
use serde_json::{Value, json};
use tungstenite::Message;



#[derive(Serialize)]
struct EncryptedWalletData {
    encrypted_phrase: String,
    salt: String,
    iv: String,
}

fn fail_log(msg: &str) {
    let mut log_opt = CHANNEL.activity_tx.borrow().clone();
    if let Some(ref mut log) = log_opt {
        log.restate_failure(msg.to_string());
        let _ = CHANNEL.activity_tx.send(log_opt.clone());
    }
}


pub async fn execute(
    _bitcoin_current_wallet: String,
    cmd: WSCommand,
) -> Result<(), String> {
    static FAILED: &str = "Error: Bitcoin wallet import failed";

    let wallet = match cmd.wallet {
        Some(w) => w,
        None => {
            fail_log(FAILED);
            return Err(FAILED.to_string());
        }
    };

  
    let (Some(xpub), Some(script_type)) = (cmd.xpub, cmd.script_type) else {
        fail_log(FAILED);
        return Err(FAILED.to_string());
    };
    let msg_json = json!({
        "command": "import_bitcoin_wallet",
        "wallet": wallet,
        "xpub": xpub,
        "script_type": script_type,
    });

    if let Some(tx) = CRYPTO_OUTGOING_TX.get() {
        if tx.send(Message::text(msg_json.to_string())).await.is_err() {
            fail_log(FAILED);
            return Err(FAILED.to_string());
        }
    }

    let mut log_opt = CHANNEL.activity_tx.borrow().clone();
    if let Some(ref mut log) = log_opt {
        log.finish("connect");
        log.start("verify");
        let _ = CHANNEL.activity_tx.send(log_opt.clone());
    }

    Ok(())
}

pub async fn process_response(
    message: Message,
    _bitcoin_current_wallet: &str,
    pending: Option<PendingWallet>,
) -> Result<(), String> {
    static FAILED: &str = "Error: Bitcoin wallet import failed";
    match message {
        Message::Text(text) => {
            let data: Value = match serde_json::from_str(&text) {
                Ok(v) => v,
                Err(e) => {
                    fail_log(&format!("{}: parse error", FAILED));
                    return Err(format!("Failed to parse JSON: {}", e));
                }
            };

        
            if let Some(err) = data.get("error").and_then(|e| e.as_str()) {
                fail_log(&format!("Error: {}", err));
                return Ok(());
            }

          
            if data.get("wallet").is_some() && data.get("used").is_none() {
                fail_log("Error: Bitcoin service is out of date — try again shortly");
                return Ok(());
            }

            if let Some(wallet) = data.get("wallet").and_then(|w| w.as_str()) {
                let balance_btc = data
                    .get("balance")
                    .and_then(|b| b.as_str())
                    .and_then(|b| b.parse::<f64>().ok())
                    .map(|b| b / 100_000_000.0)
                    .unwrap_or(0.0);

                // On-chain verified — advance to save step
                let mut log_opt = CHANNEL.activity_tx.borrow().clone();
                if let Some(ref mut log) = log_opt {
                    log.finish("verify");
                    log.start("save");
                    let _ = CHANNEL.activity_tx.send(log_opt.clone());
                }

                let Some(pending) = pending else {
                    fail_log(FAILED);
                    return Ok(());
                };

         
                if wallet != pending.address.as_str() {
                    fail_log("Error: the server named an address this wallet does not derive");
                    return Err("Import reply named an address other than the one derived".to_string());
                }

                let is_cold = pending.method == "cold";

          
                let prior_key = read_bytes("btc_encrypt.json").ok().flatten();

                if is_cold {
                  
                    let _ = remove_json("btc_encrypt.json");
                } else {
                    let wallet_data = EncryptedWalletData {
                        encrypted_phrase: pending.encrypted_phrase.clone(),
                        salt: pending.salt.clone(),
                        iv: pending.iv.clone(),
                    };

                    if let Err(e) = write_json("btc_encrypt.json", &wallet_data) {
                        let msg = format!("FS Error: {}", e);
                        fail_log(&msg);
                        return Err(msg);
                    }
                }

            
                let mut records = vec![json!({ "chain": 0, "index": 0, "address": wallet })];
                let mut members: Vec<(u32, u32, String)> = Vec::new();
                for m in data.get("used").and_then(|u| u.as_array()).into_iter().flatten() {
                    let (Some(addr), Some(chain), Some(index)) = (
                        m.get("address").and_then(|a| a.as_str()),
                        m.get("chain").and_then(|c| c.as_u64()),
                        m.get("index").and_then(|i| i.as_u64()),
                    ) else {
                        continue;
                    };
                    if addr == wallet {
                        continue;
                    }
                    let ours = crate::bridge::btc_receive_rotation::derive_member(
                        &pending.account_xpub,
                        pending.script_type,
                        chain as u32,
                        index as u32,
                    );
                    if ours.as_deref() != Some(addr) {
                      
                        match &prior_key {
                            Some(bytes) => { let _ = write_bytes("btc_encrypt.json", bytes); }
                            None => { let _ = remove_json("btc_encrypt.json"); }
                        }
                        fail_log("Error: the server named an address this wallet does not derive");
                        return Err("Import named an address outside the wallet's derivation".to_string());
                    }
                    members.push((chain as u32, index as u32, addr.to_string()));
                }
                members.sort();
                members.dedup();
                for (chain, index, addr) in &members {
                    records.push(json!({ "chain": chain, "index": index, "address": addr }));
                }
                let next_receive_index = records
                    .iter()
                    .filter(|r| r["chain"].as_u64() == Some(0))
                    .filter_map(|r| r["index"].as_u64())
                    .max()
                    .unwrap_or(0)
                    + 1;
                let next_change_index = records
                    .iter()
                    .filter(|r| r["chain"].as_u64() == Some(1))
                    .filter_map(|r| r["index"].as_u64())
                    .map(|i| i + 1)
                    .max()
                    .unwrap_or(0);
            
                let prior = read_json::<Value>("btc.json").ok().filter(|j| {
                    j.get("account_xpub").and_then(|v| v.as_str()) == Some(pending.account_xpub.as_str())
                });
                let carried_pool = prior
                    .as_ref()
                    .and_then(|j| j.get("receive_pool").cloned())
                    .unwrap_or(Value::Null);
                let carried_high = prior
                    .as_ref()
                    .and_then(|j| j.get("receive_high_water").and_then(|v| v.as_u64()));
         
                let seeded = crate::bridge::btc_receive_rotation::derive_member(
                    &pending.account_xpub,
                    pending.script_type,
                    0,
                    next_receive_index as u32,
                );
                let pool = match carried_pool {
                    Value::Array(a) if !a.is_empty() => Value::Array(a),
                    _ => match &seeded {
                        Some(address) => {
                            if !records.iter().any(|r| r["address"].as_str() == Some(address.as_str())) {
                                records.push(json!({
                                    "chain": 0,
                                    "index": next_receive_index,
                                    "address": address,
                                }));
                            }
                            json!([address])
                        }
                        None => json!([]),
                    },
                };
                let high_water = carried_high
                    .unwrap_or(next_receive_index + seeded.is_some() as u64)
                    .max(next_receive_index);

                let wallet_metadata = json!({
                    "addresses": records,
                    "next_receive_index": next_receive_index,
                    "next_change_index": next_change_index,
                    "receive_pool": pool,
                    "receive_high_water": high_water,
                    "account_xpub": pending.account_xpub,
                    "script_type": pending.script_type.tag(),
                    "private_key_deleted": is_cold,
                    "method": pending.method,
                });

                if let Err(e) = write_json("btc.json", &wallet_metadata) {
                    match prior_key {
                        Some(bytes) => { let _ = write_bytes("btc_encrypt.json", &bytes); }
                        None => { let _ = remove_json("btc_encrypt.json"); }
                    }
                    let msg = format!("Metadata Error: {}", e);
                    fail_log(&msg);
                    return Err(msg);
                }
                let _ = CHANNEL.bitcoin_wallet_tx.send((balance_btc, Some(wallet.to_string()), is_cold, crate::channel::KeyMode::from_method(&pending.method)));

              
                if let Err(e) = crate::ws::commands::getbitcoincachedbalance::apply_whole_wallet(&data) {
                    fail_log(&format!("Error: {}", e));
                    return Err(e);
                }

                let mut log_opt = CHANNEL.activity_tx.borrow().clone();
                if let Some(ref mut log) = log_opt {
                    log.finish("save");
                    let _ = CHANNEL.activity_tx.send(log_opt.clone());
                }
            } else {
                fail_log(FAILED);
            }
        }
        _ => {
            fail_log(FAILED);
        }
    }
    Ok(())
}
