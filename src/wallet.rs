use crate::channel::{CHANNEL, WSCommand};
use crate::bridge::json_storage;
use serde_json::Value;
use tokio::sync::mpsc;


#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ImportMode {
    #[default]
    Standard,
    Cold,
}


pub fn btc_address() -> Option<String> {
    json_storage::read_json::<Value>("btc.json")
        .ok()
        .and_then(|jsn| jsn.get("address").and_then(|v| v.as_str()).map(String::from))
        .filter(|a| !a.is_empty())
}

pub fn load_wallets(commands_tx: mpsc::Sender<WSCommand>) {
    if let Ok(path) = json_storage::get_config_path("xrp.json")
        && path.exists()
        && let Ok(json) = json_storage::read_json::<Value>("xrp.json")
    {
        let address = json
            .get("address")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let private_key_deleted = json
            .get("private_key_deleted")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let key_mode = crate::channel::KeyMode::from_method(
            json.get("method").and_then(|v| v.as_str()).unwrap_or(""),
        );

        if !address.is_empty() {
            let _ = CHANNEL
                .wallet_balance_tx
                .send((0.0, Some(address.clone()), private_key_deleted, key_mode));

            let command = WSCommand {
                command: "get_cached_balance".to_string(),
                wallet: Some(address.clone()),
                ..Default::default()
            };
            let _ = commands_tx.try_send(command);
        }
    }

    if let Ok(path) = json_storage::get_config_path("btc.json")
        && path.exists()
        && let Ok(json) = json_storage::read_json::<Value>("btc.json")
        && let Some(address) = json.get("address").and_then(|v| v.as_str()).filter(|a| !a.is_empty())
    {
        let private_key_deleted = json
            .get("private_key_deleted")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let key_mode = crate::channel::KeyMode::from_method(
            json.get("method").and_then(|v| v.as_str()).unwrap_or(""),
        );

        let _ = CHANNEL
            .bitcoin_wallet_tx
            .send((0.0, Some(address.to_string()), private_key_deleted, key_mode));

        let command = WSCommand {
            command: "get_bitcoin_cached_balance".to_string(),
            wallet: Some(address.to_string()),
            ..Default::default()
        };
        let _ = commands_tx.try_send(command);
    }

    CHANNEL.loaded_tx.send_if_modified(|l| !std::mem::replace(&mut l.wallets, true));
}
