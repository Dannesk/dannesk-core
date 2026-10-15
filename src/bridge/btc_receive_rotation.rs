
use crate::bridge::json_storage;
use crate::channel::CHANNEL;
use dannesk_btc_codec::bip32::Xpub;
use serde_json::{Value, json};
use std::str::FromStr;

pub const MAX_RECEIVE_POOL: usize = 5;


const GAP: u32 = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PoolRefusal {
    Full,
    OutOfGap,
    Unavailable,
}

pub fn receive_pool() -> Vec<String> {
    json_storage::read_json::<Value>("btc.json")
        .ok()
        .map(|jsn| receive_pool_of(&jsn))
        .unwrap_or_default()
}


fn receive_pool_of(jsn: &Value) -> Vec<String> {
    if let Some(arr) = jsn.get("receive_pool").and_then(|v| v.as_array()) {
        return arr.iter().filter_map(|v| v.as_str().map(String::from)).collect();
    }
    let records = crate::wallet::btc_address_records();
    jsn.get("next_receive_index")
        .and_then(|v| v.as_u64())
        .and_then(|i| records.iter().find(|r| r.chain == 0 && r.index == i as u32))
        .map(|r| vec![r.address.clone()])
        .unwrap_or_default()
}


fn high_water_of(jsn: &Value) -> u32 {
    jsn.get("receive_high_water")
        .and_then(|v| v.as_u64())
        .map(|v| v as u32)
        .unwrap_or_else(|| {
            crate::wallet::btc_address_records()
                .iter()
                .filter(|r| r.chain == 0)
                .map(|r| r.index + 1)
                .max()
                .unwrap_or(1)
        })
}


fn highest_reachable_index(jsn: &Value) -> u32 {
    match jsn.get("last_used_receive_index").and_then(|v| v.as_u64()) {
        Some(used) => used as u32 + GAP,
        None => GAP - 1,
    }
}

pub fn generate_receive_address() -> Result<String, PoolRefusal> {
    let Ok(jsn) = json_storage::read_json::<Value>("btc.json") else {
        return Err(PoolRefusal::Unavailable);
    };
    let Some(xpub) = jsn.get("account_xpub").and_then(|v| v.as_str()).filter(|s| !s.is_empty())
    else {
        return Err(PoolRefusal::Unavailable);
    };
    if receive_pool_of(&jsn).len() >= MAX_RECEIVE_POOL {
        return Err(PoolRefusal::Full);
    }
    let index = high_water_of(&jsn);
    if index > highest_reachable_index(&jsn) {
        return Err(PoolRefusal::OutOfGap);
    }
    let script_type = jsn
        .get("script_type")
        .and_then(|v| v.as_str())
        .and_then(crate::btc_script_type::BtcScriptType::from_tag)
        .unwrap_or_default();
    let Some(address) = derive_member(xpub, script_type, 0, index) else {
        return Err(PoolRefusal::Unavailable);
    };

    // Persist BEFORE handing it out: an address on screen that btc.json does
    // not record is a payment the signer could never spend.
    let recorded = address.clone();
    if json_storage::update_json("btc.json", move |data: &mut Value| {
        let Some(obj) = data.as_object_mut() else { return };
        let known = obj
            .get("addresses")
            .and_then(|a| a.as_array())
            .map(|a| a.iter().any(|r| r["address"].as_str() == Some(recorded.as_str())))
            .unwrap_or(false);
        if !known && let Some(arr) = obj.get_mut("addresses").and_then(|a| a.as_array_mut()) {
            arr.push(json!({ "chain": 0, "index": index, "address": recorded }));
        }
        let mut pool: Vec<String> = obj
            .get("receive_pool")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default();
        if !pool.iter().any(|a| a == &recorded) {
            pool.push(recorded.clone());
        }
        obj.insert("receive_pool".to_string(), json!(pool));
        obj.insert("receive_high_water".to_string(), json!(index + 1));
    })
    .is_err()
    {
        return Err(PoolRefusal::Unavailable);
    }
    send_live_list();
    Ok(address)
}

pub fn remove_receive_address(address: &str) {
    let target = address.to_string();
    let _ = json_storage::update_json("btc.json", move |data: &mut Value| {
        let Some(obj) = data.as_object_mut() else { return };
        let pool: Vec<String> = obj
            .get("receive_pool")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .filter(|a| a != &target)
                    .collect()
            })
            .unwrap_or_default();
        obj.insert("receive_pool".to_string(), json!(pool));
    });
    send_live_list();
}


pub fn ensure_change_address() -> Option<String> {
    ensure_rotating_address(1, "next_change_index")
}

fn ensure_rotating_address(chain: u32, counter_key: &'static str) -> Option<String> {
    let jsn = json_storage::read_json::<Value>("btc.json").ok()?;
    let xpub_str = jsn.get("account_xpub").and_then(|v| v.as_str()).filter(|s| !s.is_empty())?;
    let xpub = Xpub::from_str(xpub_str).ok()?;
    let script_type = jsn
        .get("script_type")
        .and_then(|v| v.as_str())
        .and_then(crate::btc_script_type::BtcScriptType::from_tag)
        .unwrap_or_default();

    let mut records = crate::wallet::btc_address_records();
    if records.is_empty() {
        return None;
    }
  
    let fallback = records
        .iter()
        .filter(|r| r.chain == chain)
        .map(|r| r.index + 1)
        .max()
        .unwrap_or(0);
    let mut index = jsn
        .get(counter_key)
        .and_then(|v| v.as_u64())
        .map(|v| v as u32)
        .unwrap_or(fallback);

    
    for _ in 0..21 {
        let offered = match records.iter().find(|r| r.chain == chain && r.index == index) {
            Some(r) => r.address.clone(),
            None => {
                let child = xpub.derive(&[chain, index]).ok()?;
                let address = script_type.address(child.public_key()).to_string();
       
                let addr = address.clone();
                if json_storage::update_json("btc.json", move |data: &mut Value| {
                    if let Some(obj) = data.as_object_mut() {
                        if let Some(arr) = obj.get_mut("addresses").and_then(|a| a.as_array_mut()) {
                            arr.push(json!({ "chain": chain, "index": index, "address": addr }));
                        }
                        obj.insert(counter_key.to_string(), json!(index));
                    }
                })
                .is_err()
                {
                    return None;
                }
                records.push(crate::wallet::BtcAddressRecord {
                    chain,
                    index,
                    address: address.clone(),
                });
          
                send_live_list();
                address
            }
        };
        if !address_used(&offered) {
         
            if jsn.get(counter_key).and_then(|v| v.as_u64()) != Some(index as u64) {
                let _ = json_storage::update_json("btc.json", move |data: &mut Value| {
                    if let Some(obj) = data.as_object_mut() {
                        obj.insert(counter_key.to_string(), json!(index));
                    }
                });
                send_live_list();
            }
            return Some(offered);
        }
        index += 1;
    }
    None
}

pub fn derive_member(
    account_xpub: &str,
    script_type: crate::btc_script_type::BtcScriptType,
    chain: u32,
    index: u32,
) -> Option<String> {
    let xpub = Xpub::from_str(account_xpub).ok()?;
    let child = xpub.derive(&[chain, index]).ok()?;
    Some(script_type.address(child.public_key()).to_string())
}


pub fn send_live_list() {
    let Ok(jsn) = json_storage::read_json::<Value>("btc.json") else { return };
    if jsn.get("account_xpub").and_then(|v| v.as_str()).is_none_or(str::is_empty) {
        return;
    }
    let records = crate::wallet::btc_address_records();
    let Some(primary) = records.first().map(|r| r.address.clone()) else { return };

    let mut list = vec![primary.clone()];
    for address in receive_pool_of(&jsn) {
        if !list.contains(&address) {
            list.push(address);
        }
    }
   
    let on_offer = jsn
        .get("next_change_index")
        .and_then(|v| v.as_u64())
        .and_then(|i| records.iter().find(|r| r.chain == 1 && r.index == i as u32));
    if let Some(r) = on_offer
        && !list.contains(&r.address)
    {
        list.push(r.address.clone());
    }
    for u in CHANNEL.btc_utxos_rx.borrow().1.iter() {
        if records.iter().any(|r| r.address == u.address) && !list.contains(&u.address) {
            list.push(u.address.clone());
        }
    }

    if let Some(tx) = crate::ws::CRYPTO_COMMANDS_TX.get() {
        let _ = tx.try_send(crate::channel::WSCommand {
            command: "subscribe_bitcoin_addresses".to_string(),
            wallet: Some(primary),
            scan: Some(list),
            ..Default::default()
        });
    }
}


fn address_used(address: &str) -> bool {
    if CHANNEL
        .btc_utxos_rx
        .borrow()
        .1
        .iter()
        .any(|u| u.address == address)
    {
        return true;
    }
    CHANNEL
        .btc_transactions_rx
        .borrow()
        .transactions
        .values()
        .any(|tx| tx.receiver_addresses.iter().any(|a| a == address))
}
