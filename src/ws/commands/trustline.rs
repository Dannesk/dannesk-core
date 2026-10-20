use crate::channel::CHANNEL;
use crate::utils::tokens;
use serde_json::Value;
use tungstenite::Message;

pub async fn execute(
    _current_wallet: String,
    _cmd: crate::channel::WSCommand,
) -> Result<(), String> {
    Ok(())
}

pub async fn process_response(message: Message, _current_wallet: &str) -> Result<(), String> {
    let Message::Text(text) = message else {
        return Err("Non-text message received".to_string());
    };

    let data: Value =
        serde_json::from_str(&text).map_err(|e| format!("Failed to parse JSON: {}", e))?;

    let command = data
        .get("command")
        .and_then(|c| c.as_str())
        .ok_or_else(|| "Missing command field".to_string())?;

    let Some(token) = tokens::by_trustline_cmd(command) else {
        return Ok(());
    };

    let (current_balance, current_has, current_limit) = CHANNEL.token(token.code);

    let trustline_limit = data
        .get("trustline_limit")
        .and_then(|l| l.as_str())
        .and_then(|l| l.parse::<f64>().ok())
        .or(current_limit);

   
    let has = data.get("has").and_then(|h| h.as_bool()).unwrap_or(current_has);
    let balance = if has { current_balance } else { 0.0 };

    CHANNEL.set_token(token.code, (balance, has, trustline_limit));

    Ok(())
}
