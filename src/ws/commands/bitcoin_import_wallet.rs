use crate::channel::{CHANNEL, PendingWallet, WSCommand};
use crate::bridge::json_storage::{read_bytes, remove_json, write_bytes, write_json};
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

    let Some(wallet) = cmd.wallet else {
        fail_log(FAILED);
        return Err(FAILED.to_string());
    };

    let msg_json = json!({
        "command": "import_bitcoin_wallet",
        "wallet": wallet,
    });

    if let Some(tx) = CRYPTO_OUTGOING_TX.get()
        && tx.send(Message::text(msg_json.to_string())).await.is_err()
    {
        fail_log(FAILED);
        return Err(FAILED.to_string());
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
    let Message::Text(text) = message else {
        fail_log(FAILED);
        return Ok(());
    };
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

    let Some(wallet) = data.get("wallet").and_then(|w| w.as_str()) else {
        fail_log(FAILED);
        return Ok(());
    };

    let balance_btc = data
        .get("balance")
        .and_then(|b| b.as_str())
        .and_then(|b| b.parse::<f64>().ok())
        .map(|b| b / 100_000_000.0)
        .unwrap_or(0.0);

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

    let wallet_metadata = json!({
        "address": wallet,
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

    if let Err(e) = crate::ws::commands::getbitcoincachedbalance::apply_wallet(&data) {
        fail_log(&format!("Error: {}", e));
        return Err(e);
    }

    let mut log_opt = CHANNEL.activity_tx.borrow().clone();
    if let Some(ref mut log) = log_opt {
        log.finish("save");
        let _ = CHANNEL.activity_tx.send(log_opt.clone());
    }
    Ok(())
}
