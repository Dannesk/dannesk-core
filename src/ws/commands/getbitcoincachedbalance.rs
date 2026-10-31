use crate::channel::{CHANNEL, WSCommand};
use crate::ws::CRYPTO_OUTGOING_TX;
use serde_json::{Value, json};
use tungstenite::Message;

pub fn fetch_payload(wallet: &str) -> Option<String> {
    let address = crate::wallet::btc_address()?;
    if address != wallet {
        return None;
    }
    Some(json!({ "command": "get_bitcoin_cached_balance", "wallet": address }).to_string())
}

pub async fn execute(
    _bitcoin_current_wallet: String,
    cmd: WSCommand,
) -> Result<(), String> {
    let wallet = cmd.wallet.ok_or("Missing wallet parameter")?;
    let payload = fetch_payload(&wallet).ok_or("Not this device's wallet")?;
    if let Some(tx) = CRYPTO_OUTGOING_TX.get() {
        let _ = tx.send(Message::text(payload)).await;
    }
    Ok(())
}

pub async fn process_response(
    message: Message,
    _bitcoin_current_wallet: &str,
) -> Result<(), String> {
    let Message::Text(text) = message else {
        return Err("Non-text message received".to_string());
    };
    let data: Value = serde_json::from_str(&text).map_err(|e| format!("Failed to parse JSON: {}", e))?;
    if data.get("command").and_then(|c| c.as_str()) != Some("get_bitcoin_cached_balance") {
        return Ok(());
    }
    if data.get("error").is_some() {
        return Ok(());
    }
    apply_wallet(&data)
}


pub(crate) fn apply_wallet(data: &Value) -> Result<(), String> {
    use crate::ws::commands::get_btc_utxos::{parse_coins, set_btc_utxos};

    let wallet = data.get("wallet").and_then(|w| w.as_str()).ok_or("Missing wallet field")?;
    if crate::wallet::btc_address().as_deref() != Some(wallet) {
        return Ok(());
    }
    if data["utxos"].is_array() {
        set_btc_utxos(wallet, parse_coins(data, wallet));
    }

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
    Ok(())
}
