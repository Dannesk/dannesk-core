use crate::channel::{CHANNEL, WSCommand};
use crate::ws::CRYPTO_OUTGOING_TX;
use serde_json::{Value, json};
use std::collections::HashMap;
use tungstenite::Message;

pub fn fetch_payload(wallet: &str) -> Option<String> {
    let jsn = crate::bridge::json_storage::read_json::<Value>("btc.json").ok()?;
    let xpub = jsn.get("account_xpub").and_then(|v| v.as_str()).filter(|s| !s.is_empty())?;
    let primary = crate::wallet::btc_address_records().first()?.address.clone();
    if primary != wallet {
        return None;
    }
    let script_type = jsn
        .get("script_type")
        .and_then(|v| v.as_str())
        .and_then(crate::btc_script_type::BtcScriptType::from_tag)
        .unwrap_or_default();
    Some(
        json!({
            "command": "get_bitcoin_cached_balance",
            "wallet": primary,
            "xpub": xpub,
            "script_type": script_type.tag(),
        })
        .to_string(),
    )
}

pub async fn execute(
    bitcoin_current_wallet: String,
    cmd: WSCommand,
) -> Result<(), String> {
    if let Some(wallet) = &cmd.wallet {
        if let Some(payload) = fetch_payload(wallet) {
            if let Some(tx) = CRYPTO_OUTGOING_TX.get() {
                let _ = tx.send(Message::text(payload)).await;
            }
            return Ok(());
        }
     
        let primary = if bitcoin_current_wallet.is_empty() { wallet.as_str() } else { bitcoin_current_wallet.as_str() };
        let msg_json = json!({ "command": "get_bitcoin_cached_balance", "wallet": wallet, "primary": primary });
        if let Some(tx) = CRYPTO_OUTGOING_TX.get() {
            let _ = tx.send(Message::text(msg_json.to_string())).await;
        }
        Ok(())
    } else {
        Err("Missing wallet parameter".to_string())
    }
}

pub async fn process_response(
    message: Message,
    _bitcoin_current_wallet: &str,
) -> Result<(), String> {
    match message {
        Message::Text(text) => {
            let data: Value =
                serde_json::from_str(&text).map_err(|e| format!("Failed to parse JSON: {}", e))?;

            let command = data.get("command").and_then(|c| c.as_str());
            if command != Some("get_bitcoin_cached_balance") {
                return Ok(());
            }

            if data.get("used").is_some() {
                return apply_whole_wallet(&data);
            }
        
            if data.get("error").is_some() {
                return Ok(());
            }

            if let Some(wallet) = data.get("wallet").and_then(|w| w.as_str()) {
                if !crate::wallet::btc_address_records().iter().any(|r| r.address == wallet) {
                    return Ok(());
                }
             
                if data.get("balance").is_none() {
                    return Err("Missing balance field".to_string());
                }

            
                let mut transactions_map = HashMap::new();
                if let Some(transactions) = data.get("transactions").and_then(|t| t.as_array()) {
                    for tx in transactions {
                        if let Some(tx_data) = crate::ws::commands::get_btc_transaction::parse_btc_tx(tx) {
                            transactions_map.insert(tx_data.txid.clone(), tx_data);
                        }
                    }
                }

                
                if data.get("utxos").is_some_and(|u| u.is_array()) {
                    crate::ws::commands::get_btc_utxos::merge_btc_utxos(
                        wallet,
                        crate::ws::commands::get_btc_utxos::parse_coins(&data, wallet),
                    );
                }

            
                CHANNEL.btc_transactions_tx.send_modify(|state| {
                    state.apply_reply(transactions_map.into_values().collect(), &data, false);
                });

                Ok(())
            } else {
                Err("Missing wallet field".to_string())
            }
        }
        _ => Err("Non-text message received".to_string()),
    }
}


pub(crate) fn apply_whole_wallet(data: &Value) -> Result<(), String> {
    use crate::bridge::btc_receive_rotation::{derive_member, send_live_list};
    use crate::ws::commands::get_btc_utxos::{parse_coins, replace_btc_utxos};

    let wallet = data.get("wallet").and_then(|w| w.as_str()).ok_or("Missing wallet field")?;
    let records = crate::wallet::btc_address_records();
    if records.first().map(|r| r.address.as_str()) != Some(wallet) {
        // Not this device's wallet (removed or replaced since the ask).
        return Ok(());
    }
    let jsn = crate::bridge::json_storage::read_json::<Value>("btc.json")
        .map_err(|e| format!("btc.json unreadable: {}", e))?;
    let xpub = jsn.get("account_xpub").and_then(|v| v.as_str()).unwrap_or_default().to_string();
    let script_type = jsn
        .get("script_type")
        .and_then(|v| v.as_str())
        .and_then(crate::btc_script_type::BtcScriptType::from_tag)
        .unwrap_or_default();

 
    let mut last_used_receive: Option<u32> = None;
    let mut fresh: Vec<(u32, u32, String)> = Vec::new();
    for m in data.get("used").and_then(|u| u.as_array()).into_iter().flatten() {
        let (Some(address), Some(chain), Some(index)) = (
            m.get("address").and_then(|a| a.as_str()),
            m.get("chain").and_then(|c| c.as_u64()),
            m.get("index").and_then(|i| i.as_u64()),
        ) else {
            continue;
        };
        if chain == 0 {
            last_used_receive = Some(last_used_receive.map_or(index as u32, |h| h.max(index as u32)));
        }
        if records.iter().any(|r| r.address == address) {
            continue;
        }
        if derive_member(&xpub, script_type, chain as u32, index as u32).as_deref() != Some(address) {
            return Err("The server named an address this wallet does not derive".to_string());
        }
        fresh.push((chain as u32, index as u32, address.to_string()));
    }
  
    let stamp_moved = last_used_receive.is_some()
        && last_used_receive
            != jsn
                .get("last_used_receive_index")
                .and_then(|v| v.as_u64())
                .map(|v| v as u32);
    if !fresh.is_empty() || stamp_moved {
        fresh.sort();
        fresh.dedup();
        crate::bridge::json_storage::update_json("btc.json", move |file: &mut Value| {
            let Some(obj) = file.as_object_mut() else { return };
            if let Some(high) = last_used_receive {
                let held = obj.get("last_used_receive_index").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
            
                if high > held || obj.get("last_used_receive_index").is_none() {
                    obj.insert("last_used_receive_index".to_string(), json!(high));
                }
            }
            for (chain, index, address) in &fresh {
                if let Some(arr) = obj.get_mut("addresses").and_then(|a| a.as_array_mut()) {
                    arr.push(json!({ "chain": chain, "index": index, "address": address }));
                }
             
                if *chain == 1 {
                    let held = obj.get("next_change_index").and_then(|v| v.as_u64()).unwrap_or(0);
                    if held <= *index as u64 {
                        obj.insert("next_change_index".to_string(), json!(*index as u64 + 1));
                    }
                }
            }
        })
        .map_err(|e| format!("btc.json write failed: {}", e))?;
    }

    let mut union = parse_coins(data, wallet);
    for f in data.get("funded").and_then(|f| f.as_array()).into_iter().flatten() {
        if let Some(addr) = f.get("address").and_then(|a| a.as_str()) {
            union.extend(parse_coins(f, addr));
        }
    }
    replace_btc_utxos(wallet, union);

    let rows: Vec<_> = data
        .get("transactions")
        .and_then(|t| t.as_array())
        .into_iter()
        .flatten()
        .filter_map(crate::ws::commands::get_btc_transaction::parse_btc_tx)
        .collect();
    CHANNEL.btc_transactions_tx.send_modify(|state| {
        state.apply_reply(rows, data, false);
    });

 
    let _ = crate::bridge::btc_receive_rotation::ensure_change_address();
    send_live_list();
    Ok(())
}
