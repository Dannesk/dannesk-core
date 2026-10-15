use crate::channel::{CHANNEL, WSCommand};
use crate::ws::CRYPTO_OUTGOING_TX;
use crate::utils::tokens;
use serde_json::{Value, json};
use tungstenite::Message;


fn is_balance_command(command: &str) -> bool {
    command == "xrp_balance" || tokens::by_balance_cmd(command).is_some()
}

// ====================== Main Logic ======================

pub async fn execute(
    _current_wallet: String,
    cmd: WSCommand,
) -> Result<(), String> {
    let wallet = cmd.wallet.as_ref().ok_or("Missing wallet parameter")?;

    if !is_balance_command(&cmd.command) {
        return Ok(());
    }

    let msg = json!({ "command": cmd.command, "wallet": wallet });

    if let Some(tx) = CRYPTO_OUTGOING_TX.get() {
        tx.send(Message::text(msg.to_string())).await.map_err(|e| e.to_string())?;
    }

    Ok(())
}

pub async fn process_response(message: Message, _current_wallet: &str) -> Result<(), String> {
    let Message::Text(text) = message else {
        return Err("Non-text message received".to_string());
    };

    let data: Value =
        serde_json::from_str(&text).map_err(|e| format!("Failed to parse JSON: {}", e))?;

    let wallet = data
        .get("wallet")
        .and_then(|w| w.as_str())
        .ok_or("Missing wallet field")?;

    let command_str = data
        .get("command")
        .and_then(|c| c.as_str())
        .ok_or("Missing command field")?;

  
    if CHANNEL.wallet_balance_rx.borrow().1.as_deref() != Some(wallet) {
        return Ok(());
    }

  
    if command_str == "xrp_balance" {
        let raw = data
            .get("balance")
            .and_then(|v| v.as_str())
            .ok_or("Missing balance field")?;
        let balance = raw
            .parse::<f64>()
            .map_err(|_| format!("Invalid balance format: {}", raw))?
            / 1_000_000.0;
        CHANNEL.wallet_balance_tx.send_modify(|state| {
            state.0 = balance;
        });
        CHANNEL.apply_xrp_account(&data);

        let exists = data
            .get("exists")
            .and_then(|v| v.as_bool())
            .unwrap_or_else(|| crate::utils::reserves::is_activated(balance));
        CHANNEL.set_xrp_exists(exists);
    } else if let Some(token) = tokens::by_balance_cmd(command_str) {
        let raw = data
            .get(token.balance_field)
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("Missing {} field", token.balance_field))?;
        let balance = raw
            .parse::<f64>()
            .map_err(|_| format!("Invalid balance format: {}", raw))?;
        CHANNEL.set_token_balance(token.code, balance);
    }

    Ok(())
}
