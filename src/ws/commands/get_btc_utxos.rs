

use crate::channel::{BtcUtxo, CHANNEL};
use serde_json::Value;
use tungstenite::Message;

pub async fn execute(
    _bitcoin_current_wallet: String,
    _cmd: crate::channel::WSCommand,
) -> Result<(), String> {
    Ok(())
}


pub fn parse_utxos(v: &Value, owner: &str) -> Vec<BtcUtxo> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|u| {
            Some(BtcUtxo {
                txid: u["txid"].as_str()?.to_string(),
                vout: u["vout"].as_u64()? as u32,
                sats: u["sats"].as_u64()?,
                height: u["height"].as_u64()?,
                address: owner.to_string(),
                spent_by: u["spent_by"].as_str().map(String::from),
            })
        })
        .collect()
}


pub fn parse_coins(frame: &Value, owner: &str) -> Vec<BtcUtxo> {
    let mut coins = parse_utxos(&frame["utxos"], owner);
    coins.extend(parse_utxos(&frame["spent"], owner));
    coins
}


pub fn merge_btc_utxos(owner: &str, set: Vec<BtcUtxo>) {
    let records = crate::wallet::btc_address_records();
    if !records.iter().any(|r| r.address == owner) {
        return;
    }
    let primary = records[0].address.clone();
    CHANNEL.btc_utxos_tx.send_modify(|(set_wallet, union)| {
     
        if set_wallet.as_deref() != Some(primary.as_str()) {
            union.clear();
            *set_wallet = Some(primary.clone());
        }
        union.retain(|u| u.address != owner);
        union.extend(set);
    });
    recompute_btc_balance();
    CHANNEL.loaded_tx.send_if_modified(|l| !std::mem::replace(&mut l.btc, true));
}


pub fn replace_btc_utxos(primary: &str, set: Vec<BtcUtxo>) {
    let records = crate::wallet::btc_address_records();
    if records.first().map(|r| r.address.as_str()) != Some(primary) {
        return;
    }
    let set: Vec<BtcUtxo> = set
        .into_iter()
        .filter(|u| records.iter().any(|r| r.address == u.address))
        .collect();
    CHANNEL.btc_utxos_tx.send_modify(|(set_wallet, union)| {
        *set_wallet = Some(primary.to_string());
        *union = set;
    });
    recompute_btc_balance();
    CHANNEL.loaded_tx.send_if_modified(|l| !std::mem::replace(&mut l.btc, true));
}


pub fn recompute_btc_balance() {
    let sats: u64 = CHANNEL
        .btc_utxos_rx
        .borrow()
        .1
        .iter()
        .filter(|u| u.height > 0)
        .map(|u| u.sats)
        .sum();
    CHANNEL.bitcoin_wallet_tx.send_modify(|state| {
        state.0 = sats as f64 / 100_000_000.0;
    });
}

pub async fn process_response(
    message: Message,
    _bitcoin_current_wallet: &str,
) -> Result<(), String> {
    let Message::Text(text) = message else {
        return Err("Non-text message received".to_string());
    };
    let data: Value =
        serde_json::from_str(&text).map_err(|e| format!("Failed to parse JSON: {}", e))?;
    if data.get("command").and_then(|c| c.as_str()) != Some("btc_utxos") {
        return Ok(());
    }
    let wallet = data
        .get("wallet")
        .and_then(|w| w.as_str())
        .ok_or("Missing wallet field")?;
    merge_btc_utxos(wallet, parse_coins(&data, wallet));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;


    #[test]
    fn both_lists_of_a_frame_become_one_marked_set() {
        let frame = json!({
            "utxos": [{ "txid": "aa", "vout": 0, "sats": 5_923, "height": 965_711 }, { "txid": "broken" }],
            "spent": [{ "txid": "bb", "vout": 1, "sats": 5_802, "height": 969_974, "spent_by": "cc" }],
        });
        let coins = parse_coins(&frame, "bc1qme");
        assert_eq!(coins.len(), 2);
        assert_eq!(coins[0].txid, "aa");
        assert_eq!(coins[0].spent_by, None);
        assert_eq!(coins[1].txid, "bb");
        assert_eq!(coins[1].spent_by.as_deref(), Some("cc"));
        assert!(coins.iter().all(|c| c.address == "bc1qme"));
        let old = json!({ "utxos": [{ "txid": "aa", "vout": 0, "sats": 1, "height": 1 }] });
        assert_eq!(parse_coins(&old, "bc1qme").len(), 1);
    }
}
