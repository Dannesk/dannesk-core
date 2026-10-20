use crate::channel::{CHANNEL, WSCommand};
use crate::ws::CRYPTO_OUTGOING_TX;
use serde_json::{Value, json};
use tungstenite::Message;

pub async fn execute(
    _current_wallet: String,
    cmd: WSCommand,
) -> Result<(), String> {
    if let Some(wallet) = &cmd.wallet {
        let msg_json = json!({ "command": "get_cached_balance", "wallet": wallet });
        if let Some(tx) = CRYPTO_OUTGOING_TX.get() {
            let _ = tx.send(Message::text(msg_json.to_string())).await;
        }
        Ok(())
    } else {
        Err("Missing wallet parameter".to_string())
    }
}

pub async fn process_response(message: Message, _current_wallet: &str) -> Result<(), String> {
    match message {
        Message::Text(text) => {
            let data: Value =
                serde_json::from_str(&text).map_err(|e| format!("Failed to parse JSON: {}", e))?;

            if data.get("command").and_then(|c| c.as_str()) != Some("get_cached_balance") {
                return Ok(());
            }

            if let Some(error) = data.get("error").and_then(|e| e.as_str()) {
                return Err(format!("Server error: {}", error));
            }

            let wallet = data
                .get("wallet")
                .and_then(|w| w.as_str())
                .ok_or_else(|| "Missing wallet field".to_string())?;

            if CHANNEL.wallet_balance_rx.borrow().1.as_deref() != Some(wallet) {
                return Ok(());
            }

            let balance_xrp = data
                .get("balance")
                .and_then(|b| b.as_str())
                .and_then(|b| b.parse::<f64>().ok())
                .map(|b| b / 1_000_000.0)
                .unwrap_or(0.0);

            let transactions_data = if let Some(tx_value) = data.get("transactions") {
                if tx_value.is_null() {
                    Vec::new()
                } else if let Some(tx_array) = tx_value.as_array() {
                
                    tx_array
                        .iter()
                        .filter_map(super::get_transaction::parse_tx)
                        .collect()
                } else {
                    return Err("Invalid transaction format".to_string());
                }
            } else {
                Vec::new()
            };

            let (_, _, private_key_deleted, key_mode) = *CHANNEL.wallet_balance_rx.borrow();
            let _ = CHANNEL.wallet_balance_tx.send((balance_xrp, Some(wallet.to_string()), private_key_deleted, key_mode));
        
            let exists = data
                .get("exists")
                .and_then(|v| v.as_bool())
                .unwrap_or_else(|| crate::utils::reserves::is_activated(balance_xrp));
            CHANNEL.set_xrp_exists(exists);
            CHANNEL.apply_xrp_account(&data);
       
            for token in crate::utils::tokens::TOKENS {
                let balance = data
                    .get(token.balance_field)
                    .and_then(|v| v.as_str())
                    .and_then(|v| v.parse::<f64>().ok())
                    .unwrap_or(0.0);
                let has = data
                    .get(token.has_field)
                    .and_then(|h| h.as_bool())
                    .unwrap_or(false);
                let limit = data
                    .get(token.trustline_limit_field)
                    .and_then(|l| l.as_str())
                    .and_then(|l| l.parse::<f64>().ok());
                CHANNEL.set_token(token.code, (balance, has, limit));
            }

         
            CHANNEL.transactions_tx.send_modify(|state| state.apply_reply(transactions_data, &data, None));

            CHANNEL.loaded_tx.send_if_modified(|l| !std::mem::replace(&mut l.xrp, true));

            Ok(())
        }
        _ => Err("Non-text message received".to_string()),
    }
}
