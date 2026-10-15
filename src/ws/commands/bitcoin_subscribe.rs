
use crate::channel::WSCommand;
use crate::ws::CRYPTO_OUTGOING_TX;
use serde_json::json;
use tungstenite::Message;

pub async fn execute(_bitcoin_current_wallet: String, cmd: WSCommand) -> Result<(), String> {
    let (Some(wallet), Some(addresses)) = (cmd.wallet, cmd.scan) else {
        return Err("Missing wallet or address list".to_string());
    };
    let msg_json = json!({
        "command": "subscribe_bitcoin_addresses",
        "wallet": wallet,
        "addresses": addresses,
    });
    if let Some(tx) = CRYPTO_OUTGOING_TX.get() {
        let _ = tx.send(Message::text(msg_json.to_string())).await;
    }
    Ok(())
}
