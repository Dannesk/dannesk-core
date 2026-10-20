use crate::channel::{CHANNEL, WSCommand};
use crate::bridge::json_storage;
use serde_json::{Value, json};
use tokio::sync::mpsc;


#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ImportMode {
    #[default]
    Standard,
    Cold,
}


#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BtcAddressRecord {
    pub chain: u32,
    pub index: u32,
    pub address: String,
}


pub fn btc_address_records() -> Vec<BtcAddressRecord> {
    let Ok(jsn) = json_storage::read_json::<Value>("btc.json") else {
        return Vec::new();
    };
    records_from(&jsn)
}

fn records_from(jsn: &Value) -> Vec<BtcAddressRecord> {
    if let Some(arr) = jsn.get("addresses").and_then(|v| v.as_array()) {
        return arr
            .iter()
            .filter_map(|r| {
                Some(BtcAddressRecord {
                    chain: r.get("chain")?.as_u64()? as u32,
                    index: r.get("index")?.as_u64()? as u32,
                    address: r.get("address")?.as_str()?.to_string(),
                })
            })
            .collect();
    }
    jsn.get("address")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|a| {
            vec![BtcAddressRecord {
                chain: 0,
                index: 0,
                address: a.to_string(),
            }]
        })
        .unwrap_or_default()
}


fn migrate_btc_json(jsn: &Value) {
    if jsn.get("addresses").is_some() {
        return;
    }
    let records = records_from(jsn);
    if records.is_empty() {
        return;
    }
    let _ = json_storage::update_json("btc.json", |data: &mut Value| {
        if let Some(obj) = data.as_object_mut() {
            let addr = obj.remove("address");
            obj.insert(
                "addresses".to_string(),
                json!([{ "chain": 0, "index": 0, "address": addr.as_ref().and_then(|a| a.as_str()).unwrap_or_default() }]),
            );
            obj.insert("next_receive_index".to_string(), json!(1));
        }
    });
}

pub fn load_wallets(commands_tx: mpsc::Sender<WSCommand>) {
      if let Ok(path) = json_storage::get_config_path("xrp.json")
    && path.exists()
        && let Ok(json) = json_storage::read_json::<Value>("xrp.json") {


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
                    let _ =
                        CHANNEL
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
  && let Ok(json) = json_storage::read_json::<Value>("btc.json") {

               
                migrate_btc_json(&json);
                let records = records_from(&json);

                let private_key_deleted = json
                    .get("private_key_deleted")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let key_mode = crate::channel::KeyMode::from_method(
                    json.get("method").and_then(|v| v.as_str()).unwrap_or(""),
                );

               
                if let Some(primary) = records.first().map(|r| r.address.clone()) {
                    let _ =
                        CHANNEL
                            .bitcoin_wallet_tx
                            .send((0.0, Some(primary), private_key_deleted, key_mode));

                
                    let has_key = json
                        .get("account_xpub")
                        .and_then(|v| v.as_str())
                        .is_some_and(|s| !s.is_empty());
                    for record in records.iter().take(if has_key { 1 } else { records.len() }) {
                        let command = WSCommand {
                            command: "get_bitcoin_cached_balance".to_string(),
                            wallet: Some(record.address.clone()),
                            ..Default::default()
                        };
                        let _ = commands_tx.try_send(command);
                    }
                }
    }

    CHANNEL.loaded_tx.send_if_modified(|l| !std::mem::replace(&mut l.wallets, true));
}
