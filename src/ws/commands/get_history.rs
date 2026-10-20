

use crate::channel::{CHANNEL, HistoryList, WSCommand};
use crate::ws::CRYPTO_OUTGOING_TX;
use serde_json::{Value, json};
use tungstenite::Message;

pub async fn execute(_current_wallet: String, cmd: WSCommand) -> Result<(), String> {
    let wallet = cmd.wallet.ok_or("Missing wallet parameter")?;
    let kind = cmd.history_kind.ok_or("Missing history kind")?;
    let msg_json = json!({
        "command": "get_history",
        "wallet": wallet,
        "kind": kind,
        "offset": cmd.history_offset.unwrap_or(0),
    });
    if let Some(tx) = CRYPTO_OUTGOING_TX.get() {
        let _ = tx.send(Message::text(msg_json.to_string())).await;
    }
    Ok(())
}

pub async fn process_response(message: Message, _current_wallet: &str) -> Result<(), String> {
    let Message::Text(text) = message else {
        return Err("Non-text message received".to_string());
    };
    let data: Value = serde_json::from_str(&text).map_err(|e| format!("Failed to parse JSON: {}", e))?;
    if data.get("command").and_then(|c| c.as_str()) != Some("get_history") {
        return Ok(());
    }
    let wallet = data.get("wallet").and_then(|w| w.as_str()).ok_or("Missing wallet field")?;
    if CHANNEL.wallet_balance_rx.borrow().1.as_deref() != Some(wallet) {
        return Ok(());
    }
    let list = match data.get("kind").and_then(|k| k.as_str()) {
        Some("transactions") => HistoryList::XrpTransactions,
        Some("orders") => HistoryList::XrpOrders,
        _ => return Err("Missing history kind".to_string()),
    };
   
    if let Some(error) = data.get("error").and_then(|e| e.as_str()) {
        CHANNEL.transactions_tx.send_modify(|state| state.page_mut(list).fail_any());
        return Err(format!("Server error: {}", error));
    }
    let rows: Vec<_> = data
        .get("transactions")
        .and_then(|t| t.as_array())
        .into_iter()
        .flatten()
        .filter_map(super::get_transaction::parse_tx)
        .collect();
    CHANNEL.transactions_tx.send_modify(|state| state.apply_reply(rows, &data, Some(list)));
    Ok(())
}
