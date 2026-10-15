use serde_json::Value;
use tungstenite::Message;

pub async fn execute(
    _bitcoin_current_wallet: String,
    _cmd: crate::channel::WSCommand,
) -> Result<(), String> {
    Ok(())
}

pub async fn process_response(
    message: Message,
    _bitcoin_current_wallet: &str,
) -> Result<(), String> {
    match message {
        Message::Text(text) => {
            let data: Value =
                serde_json::from_str(&text).map_err(|e| format!("Failed to parse JSON: {}", e))?;

            let wallet = data
                .get("wallet")
                .and_then(|w| w.as_str())
                .ok_or("Missing wallet field")?;

            let command = data.get("command").and_then(|c| c.as_str());
            if command != Some("btc_balance") {
                return Ok(());
            }

       
            let _ = wallet;
            crate::ws::commands::get_btc_utxos::recompute_btc_balance();
        }
        _ => {
            return Err("Non-text message received".to_string());
        }
    }
    Ok(())
}
