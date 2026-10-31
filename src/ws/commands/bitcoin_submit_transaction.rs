use crate::channel::{BtcUtxo, CHANNEL, WSCommand};
use crate::ws::commands::{
    bitcoin_auth, bitcoin_payment, bitcoin_transaction_sender, bitcoin_validation,
};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use tungstenite::Message;


fn in_flight() -> &'static Mutex<HashMap<String, InFlight>> {
    static MAP: OnceLock<Mutex<HashMap<String, InFlight>>> = OnceLock::new();
    MAP.get_or_init(Default::default)
}

struct InFlight {
    wallet: String,
    tx_hex: String,
 
    replaces: Option<String>,
}

pub(crate) fn stash_in_flight(wallet: &str, tx_hex: &str, replaces: Option<&str>) {
    use dannesk_btc_codec::tx::Transaction;
    if let Ok(tx) = Transaction::from_hex(tx_hex) {
        in_flight().lock().unwrap().insert(
            tx.txid().to_string(),
            InFlight {
                wallet: wallet.to_string(),
                tx_hex: tx_hex.to_string(),
                replaces: replaces.map(|s| s.to_string()),
            },
        );
    }
}


fn apply_local_spend(txid: &str) {
    use dannesk_btc_codec::address::Address;
    use dannesk_btc_codec::tx::Transaction;
    let Some(InFlight { wallet, tx_hex, replaces }) = in_flight().lock().unwrap().remove(txid) else {
        return;
    };
    let Ok(tx) = Transaction::from_hex(&tx_hex) else {
        return;
    };
    let spent: Vec<(String, u32)> = tx
        .input
        .iter()
        .map(|i| (i.previous_output.txid.to_string(), i.previous_output.vout))
        .collect();
 
    let owner_of = |spk: &[u8]| -> Option<String> {
        Address::from_script(spk)
            .map(|a| a.to_string())
            .filter(|a| *a == wallet)
    };
    let ours = |spk: &[u8]| owner_of(spk).is_some();

    let mut spent_sats = 0u64;
   
    let mut spent_coins: Vec<crate::channel::BtcRbfInput> = Vec::new();
    CHANNEL.btc_utxos_tx.send_modify(|(set_wallet, set)| {
        if set_wallet.as_deref() != Some(wallet.as_str()) {
            return;
        }
        for u in set.iter_mut() {
            if spent.iter().any(|(t, v)| *t == u.txid && *v == u.vout) {
                spent_sats += u.sats;
                spent_coins.push(crate::channel::BtcRbfInput {
                    txid: u.txid.clone(),
                    vout: u.vout,
                    sats: u.sats,
                    address: Some(u.address.clone()),
                });
         
                u.spent_by = Some(txid.to_string());
            }
        }
      
        set.retain(|u| replaces.as_deref() != Some(u.txid.as_str()));
        for (vout, out) in tx.output.iter().enumerate() {
           
            let held = set.iter().any(|u| u.txid == txid && u.vout == vout as u32);
            if let Some(owner) = owner_of(&out.script_pubkey)
                && !held
            {
                set.push(BtcUtxo {
                    txid: txid.to_string(),
                    vout: vout as u32,
                    sats: out.value,
                    height: 0,
                    address: owner,
                    spent_by: None,
                });
            }
        }
    });
    crate::ws::commands::get_btc_utxos::recompute_btc_balance();

    let sent_sats: u64 = tx
        .output
        .iter()
        .filter(|o| !ours(&o.script_pubkey))
        .map(|o| o.value)
        .sum();
    let fee_sats = spent_sats.saturating_sub(tx.output.iter().map(|o| o.value).sum());
    let receivers: Vec<String> = tx
        .output
        .iter()
        .filter(|o| !ours(&o.script_pubkey))
        .filter_map(|o| Address::from_script(&o.script_pubkey).map(|a| a.to_string()))
        .collect();
  
    let inputs = if spent_coins.len() == tx.input.len() { spent_coins } else { Vec::new() };
    let outputs: Vec<crate::channel::BtcRbfOutput> = tx
        .output
        .iter()
        .enumerate()
        .map(|(vout, o)| crate::channel::BtcRbfOutput {
            vout: vout as u32,
            sats: o.value,
            address: Address::from_script(&o.script_pubkey).map(|a| a.to_string()),
            spk: Some(hex::encode(&o.script_pubkey)),
        })
        .collect();
    let vsize = Some(tx.vsize());
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .to_string();
    CHANNEL.btc_transactions_tx.send_modify(|state| {
        state
            .transactions
            .entry(txid.to_string())
            .or_insert(crate::channel::BtcTransactionData {
                txid: txid.to_string(),
                status: crate::channel::BitcoinTransactionStatus::Pending,
                amount: format!("{:.8}", sent_sats as f64 / 1e8),
                fees: fee_sats.to_string(),
                receiver_addresses: receivers,
                sender_addresses: vec![wallet],
                timestamp: now,
                confirmed_at: None,
                dropped_at: None,
                block_height: None,
                replaced_by: None,
                inputs,
                outputs,
                vsize,
            });
    });
}


pub fn report_unsent() {
    discard_in_flight(None);
    fail_log("No connection to the network — nothing was sent.");
}


pub(crate) fn discard_in_flight(txid: Option<&str>) {
    let mut map = in_flight().lock().unwrap();
    match txid {
        Some(t) => {
            map.remove(t);
        }
       
        None => map.clear(),
    }
}


pub(crate) fn dispatched() {
    CHANNEL.btc_send_dispatched_tx.send_modify(|n| *n = n.wrapping_add(1));
}

pub(crate) fn fail_log(msg: &str) {
    let mut log_opt = CHANNEL.activity_tx.borrow().clone();
    if let Some(ref mut log) = log_opt {
        log.restate_failure(msg.to_string());
        let _ = CHANNEL.activity_tx.send(log_opt.clone());
    }
}

pub(crate) fn advance_log(done: &'static str, next: &'static str) {
    let mut log_opt = CHANNEL.activity_tx.borrow().clone();
    if let Some(ref mut log) = log_opt {
        log.finish(done);
        log.start(next);
        let _ = CHANNEL.activity_tx.send(log_opt.clone());
    }
}

pub(crate) fn finish_log(id: &'static str) {
    let mut log_opt = CHANNEL.activity_tx.borrow().clone();
    if let Some(ref mut log) = log_opt {
        log.finish(id);
        let _ = CHANNEL.activity_tx.send(log_opt.clone());
    }
}

pub async fn execute(
    bitcoin_current_wallet: String,
    mut cmd: WSCommand,
) -> Result<(), String> {
    let (tx_type, wallet, _passphrase) =
        bitcoin_validation::validate_inputs(&cmd, &bitcoin_current_wallet)
            .map_err(|e| {
                fail_log(&e);
                e
            })?;

 
    let fee = cmd
        .fee
        .as_ref()
        .ok_or_else(|| {
            fail_log("No transaction fee specified");
            "Error: No transaction fee specified".to_string()
        })?
        .to_string();

    if fee.parse::<u32>().is_err() {
        fail_log("Invalid transaction fee");
        return Err("Error: Invalid transaction fee format".to_string());
    }

    advance_log("init", "auth");

    let passphrase = cmd.passphrase.take();
    let seed = cmd.seed.take();
    let bip39 = cmd.bip39.take();
    let wallet_clone = wallet.clone();
    let auth_result = tokio::task::spawn_blocking(move || {
        bitcoin_auth::authenticate_wallet(passphrase, seed, bip39, &wallet_clone)
    })
    .await
    .map_err(|e| format!("Internal thread error: {}", e));

    let wallet_obj = match auth_result {
        Ok(Ok(w)) => w,
      
        Ok(Err(e)) => {
            fail_log("Authentication failed");
            return Err(e);
        }
        Err(e) => {
            fail_log("Authentication failed");
            return Err(e);
        }
    };

    advance_log("auth", "build");

   
    let utxos = bitcoin_payment::eligible_utxos(&wallet).map_err(|e| {
        fail_log("No spendable coins in view");
        e
    })?;

    let tx_hex = match bitcoin_payment::construct_transaction(&wallet_obj, &cmd, &tx_type, utxos, fee).await
    {
        Ok(h) => h,
        Err(e) => {
            fail_log("Failed to build transaction");
            return Err(e);
        }
    };

    advance_log("build", "broadcast");

 
    if !*CHANNEL.btc_ws_status_rx.borrow() {
        fail_log("No connection to the network — nothing was sent.");
        return Err("Error: no connection to the network".to_string());
    }

   
    stash_in_flight(&wallet, &tx_hex, None);

    
    dispatched();

    if let Err(e) =
        bitcoin_transaction_sender::send_transaction(&wallet, &tx_type, tx_hex, None).await
    {
        discard_in_flight(None);
        fail_log(&e);
        return Err(e);
    }

    advance_log("broadcast", "confirm");

    Ok(())
}

pub async fn process_response(
    message: Message,
    _bitcoin_current_wallet: &str,
) -> Result<(), String> {
    match message {
        Message::Text(text) => {
            let data: Value = match serde_json::from_str(&text) {
                Ok(v) => v,
                Err(_) => {
                    fail_log("Transaction failed");
                    return Err("Failed to parse response".to_string());
                }
            };

            let command = data.get("command").and_then(|c| c.as_str());

            // Server received the blob and is processing — no step change needed
            if command == Some("bitcoin_submit_transaction")
                && data.get("status").and_then(|s| s.as_str()) == Some("submitted")
            {
                return Ok(());
            }

            if command == Some("bitcoin_transaction_ack") {
                return Ok(());
            }

            if command != Some("submit_bitcoin_transaction_response") {
                return Ok(());
            }

            let result = match data.get("result") {
                Some(r) => r,
                None => {
                    fail_log("Transaction failed");
                    return Err("Missing result".to_string());
                }
            };

            if result.get("status").and_then(|s| s.as_str()) != Some("success") {
                discard_in_flight(result.get("txid").and_then(|t| t.as_str()));
                let msg = result
                    .get("error")
                    .and_then(|e| e.get("message"))
                    .and_then(|m| m.as_str())
                    .unwrap_or("Transaction failed")
                    .to_string();
                fail_log(&msg);
                return Err(msg);
            }

          
            if let Some(txid) = result.get("txid").and_then(|t| t.as_str()) {
                apply_local_spend(txid);
            }

            finish_log("confirm");

            Ok(())
        }
        _ => Ok(()),
    }
}
