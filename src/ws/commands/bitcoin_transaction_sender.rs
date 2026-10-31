use crate::ws::CRYPTO_OUTGOING_TX;
use serde_json::json;
use tungstenite::Message;
use uuid::Uuid;


pub async fn send_transaction(
    wallet: &str,
    tx_type: &str,
    tx_hex: String,
    replaces: Option<&str>,
) -> Result<(), String> {
    let tx_id = Uuid::new_v4().to_string();
    let mut msg_json = json!({
        "command": "submit_bitcoin_transaction",
        "address": wallet,
        "tx_type": tx_type,
        "tx_id": tx_id,
        "signed_blob": json!({ "tx_hex": tx_hex })
    });
    if let Some(old) = replaces {
        msg_json["replaces"] = json!(old);
    }

    if let Some(tx) = CRYPTO_OUTGOING_TX.get() {
        if tx.send(Message::text(msg_json.to_string())).await.is_err() {
            return Err("Failed to send Bitcoin transaction".to_string());
        }
    } else {
        return Err("Internal error: outgoing channel not initialized".to_string());
    }

    Ok(())
}
