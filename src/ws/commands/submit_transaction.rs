use crate::channel::{ActivityStepState, CHANNEL, WSCommand};
use crate::ws::commands::{transaction_builder, transaction_sender, validation, wallet_auth};
use serde_json::Value;
use sha2::{Digest, Sha512};
use tungstenite::Message;

fn fail_log(msg: &str) {
    let mut log_opt = CHANNEL.activity_tx.borrow().clone();
    if let Some(ref mut log) = log_opt {
        log.restate_failure(msg.to_string());
        let _ = CHANNEL.activity_tx.send(log_opt.clone());
    }
}

fn advance_log(done: &'static str, next: &'static str) {
    let mut log_opt = CHANNEL.activity_tx.borrow().clone();
    if let Some(ref mut log) = log_opt {
        log.finish(done);
        log.start(next);
        let _ = CHANNEL.activity_tx.send(log_opt.clone());
    }
}

fn finish_log(id: &'static str) {
    let mut log_opt = CHANNEL.activity_tx.borrow().clone();
    if let Some(ref mut log) = log_opt {
        log.finish(id);
        let _ = CHANNEL.activity_tx.send(log_opt.clone());
    }
}


fn finish_log_noted(id: &'static str, note: String) {
    let mut log_opt = CHANNEL.activity_tx.borrow().clone();
    if let Some(ref mut log) = log_opt {
        log.finish(id);
        log.note(note);
        let _ = CHANNEL.activity_tx.send(log_opt.clone());
    }
}


fn ensure_confirming() {
    let mut log_opt = CHANNEL.activity_tx.borrow().clone();
    if let Some(ref mut log) = log_opt {
        let broadcasting = log.steps.iter().any(|s| {
            s.id == "broadcast"
                && matches!(s.state, ActivityStepState::Active { .. } | ActivityStepState::Stalled { .. })
        });
        if broadcasting {
            log.finish("broadcast");
            log.start("confirm");
            let _ = CHANNEL.activity_tx.send(log_opt.clone());
        }
    }
}


pub fn report_unsent() {
    fail_log("No connection to the network — nothing was sent.");
    release_pending();
}

pub async fn execute(
    current_wallet: String,
    mut cmd: WSCommand,
) -> Result<(), String> {
    let mut wallet_copy = current_wallet;

    let (tx_type, wallet, _passphrase) = validation::validate_inputs(&cmd, &mut wallet_copy)
        .map_err(|e| {
            fail_log(&e);
            release_pending();
            e
        })?;

    advance_log("init", "auth");

  
    let passphrase = cmd.passphrase.take();
    let seed = cmd.seed.take();
    let bip39 = cmd.bip39.take();
    let wallet_clone = wallet.clone();
    let auth_result = tokio::task::spawn_blocking(move || {
        wallet_auth::authenticate_wallet(passphrase, seed, bip39, &wallet_clone)
    })
    .await
    .map_err(|e| format!("Internal thread error: {}", e));

    let wallet_obj = match auth_result {
        Ok(Ok(w)) => w,
        Ok(Err(e)) => {
            fail_log("Authentication failed");
            release_pending();
            return Err(e);
        }
        Err(e) => {
            fail_log("Authentication failed");
            release_pending();
            return Err(e);
        }
    };

    advance_log("auth", "build");

    let env = ledger_env(&wallet).map_err(|e| {
        fail_log(&e);
        release_pending();
        e
    })?;

    let tx_blob = match transaction_builder::construct_blob(
        &wallet_obj,
        &cmd,
        &tx_type,
        env.sequence,
        env.fee,
        env.last_ledger_sequence,
    )
    .await
    {
        Ok(b) => b,
        Err(e) => {
            fail_log("Failed to build transaction");
            release_pending();
            return Err(e);
        }
    };

    advance_log("build", "broadcast");

   
    if !*CHANNEL.relay_ws_status_rx.borrow() {
        fail_log("No connection to the network — nothing was sent.");
        release_pending();
        return Err("Error: no connection to the network".to_string());
    }

    let tx_id = match transaction_sender::send_transaction(&wallet, &tx_type, tx_blob.clone()).await {
        Ok(id) => id,
        Err(e) => {
            fail_log(&e);
            release_pending();
            return Err(e);
        }
    };

  
    if tx_type == "offer_create"
        && let Some(hash) = transaction_hash(&tx_blob)
    {
        let mut pending = CHANNEL.pending_trade_rx.borrow().clone();
        if let Some(quote) = pending.as_mut() {
            quote.last_ledger = Some(env.last_ledger_sequence);
            quote.sequence = Some(env.sequence);
            quote.hash = Some(hash.clone());
            quote.tx_id = Some(tx_id);
            let _ = CHANNEL.pending_trade_tx.send(pending);
        }
    }

 

    Ok(())
}


fn release_pending() {
    if CHANNEL.pending_trade_rx.borrow().is_some() {
        let _ = CHANNEL.pending_trade_tx.send(None);
    }
}

pub async fn process_response(message: Message, _current_wallet: &str) -> Result<(), String> {
    match message {
        Message::Text(text) => {
            let data: Value = match serde_json::from_str(&text) {
                Ok(v) => v,
                Err(_) => {
                    fail_log("Transaction failed");
                    return Err("Failed to parse response".to_string());
                }
            };

         
            if data.get("command").and_then(|c| c.as_str()) == Some("submit_transaction")
                && data.get("status").and_then(|s| s.as_str()) == Some("submitted")
            {
                ensure_confirming();
                return Ok(());
            }

            if data.get("command").and_then(|c| c.as_str()) == Some("transaction_ack") {
                return Ok(());
            }

            if data.get("command").and_then(|c| c.as_str()) != Some("submit_transaction_response") {
                return Ok(());
            }

          
            ensure_confirming();

            let result = match data.get("result") {
                Some(r) => r,
                None => {
                    fail_log("Transaction failed");
                    return Err("Missing result".to_string());
                }
            };

    
            if data.get("tx_type").and_then(|t| t.as_str()) == Some("offer_create")
                && let Some(status) = result.get("status").and_then(|s| s.as_str())
            {
                let sentence = outcome_sentence(status, result);
                settle_pending(data.get("tx_id").and_then(|v| v.as_str()));
                if status == "killed" || status == "failed" {
                    fail_log(&sentence);
                    return Err(sentence);
                }
                finish_log_noted("confirm", sentence);
                return Ok(());
            }

            let transaction_result = result.get("transaction_result").and_then(|t| t.as_str());
            if transaction_result != Some("tesSUCCESS") {
              
                let msg = if transaction_result == Some("tecKILLED") {
                    "Order could not fill at that price".to_string()
                } else if let Some(said) = result.get("message").and_then(|m| m.as_str()) {
                    format!("Rejected by the network \u{2014} {}", said.trim_end_matches('.'))
                } else {
                    "Transaction failed".to_string()
                };
                fail_log(&msg);
                return Err(msg);
            }

            finish_log("confirm");

            Ok(())
        }
        _ => Ok(()),
    }
}


fn settle_pending(tx_id: Option<&str>) {
    let pending = CHANNEL.pending_trade_rx.borrow().clone();
    let Some(quote) = pending else { return };
    if quote.tx_id.as_deref() != tx_id || tx_id.is_none() {
        return;
    }
    let _ = CHANNEL.pending_trade_tx.send(None);
}


fn transaction_hash(tx_blob_hex: &str) -> Option<String> {
    let bytes = hex::decode(tx_blob_hex).ok()?;
    let mut hasher = Sha512::new();
    hasher.update([0x54, 0x58, 0x4E, 0x00]);
    hasher.update(&bytes);
    Some(hex::encode(&hasher.finalize()[0..32]).to_uppercase())
}


fn trim(v: &Value, key: &str) -> String {
    let raw = v.get(key).and_then(|x| x.as_str()).unwrap_or("");
    if !raw.contains('.') {
        return raw.to_string();
    }
    raw.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn ccy<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(|x| x.as_str()).unwrap_or("")
}


fn outcome_sentence(status: &str, r: &Value) -> String {
    let (filled, fc) = (trim(r, "filled"), ccy(r, "filled_currency"));
    let (received, rc) = (trim(r, "received"), ccy(r, "received_currency"));
    let (asked, ac) = (trim(r, "requested"), ccy(r, "requested_currency"));

    let (amount, unit, price_unit) = if rc == "XRP" {
        (received.as_str(), rc, fc)
    } else {
        (filled.as_str(), fc, rc)
    };
    let at = match r.get("fill_price").and_then(|x| x.as_str()) {
        Some(p) if !p.is_empty() => format!(" at {} {}", p, price_unit),
        _ => String::new(),
    };
    let traded = format!("{} {}{}", amount, unit, at);

    match status {
        "success" => format!("Filled {}.", traded),
        "partial" => {
            let of = if ac == unit {
                format!("Filled {} of {} {}{}", amount, asked, unit, at)
            } else if ac == fc {
                format!("Filled {} of {} {} \u{2014} {}", filled, asked, fc, traded)
            } else if ac == rc {
                format!("Received {} of {} {} \u{2014} {}", received, asked, rc, traded)
            } else {
                format!("Filled {}", traded)
            };
            format!("{}. The rest did not fill and is gone.", of)
        }
       
        "killed" => "Nothing filled \u{2014} the book moved before the order landed. Only the network fee was charged.".to_string(),
        "pending" if filled.parse::<f64>().unwrap_or(0.0) > 0.0 => {
            format!("Filled {}. The rest is resting on the book.", traded)
        }
        "pending" => "Nothing filled yet \u{2014} the order is resting on the book.".to_string(),
     
        _ => match r.get("message").and_then(|m| m.as_str()) {
            Some(said) => format!(
                "The ledger rejected the order \u{2014} {}. Nothing moved.",
                said.trim_end_matches('.')
            ),
            None => "The ledger rejected the order \u{2014} nothing moved.".to_string(),
        },
    }
}

struct LedgerEnv {
    sequence: u32,
    /// Drops.
    fee: u64,
    last_ledger_sequence: u32,
}


pub const LAST_LEDGER_OFFSET: u32 = 20;

fn ledger_env(wallet: &str) -> Result<LedgerEnv, String> {
   
    let wallet_loaded = CHANNEL.wallet_balance_rx.borrow().1.as_deref() == Some(wallet);
    let account = *CHANNEL.xrp_account_rx.borrow();
    let node = CHANNEL.xrp_node_rx.borrow().clone();

    let sequence = account
        .sequence
        .filter(|_| wallet_loaded)
        .ok_or_else(|| "Wallet data not loaded yet — try again shortly".to_string())?;

    let fee = node
        .open_ledger_fee
        .ok_or_else(|| "Node data not loaded yet — try again shortly".to_string())?;
    let ledger_index = node
        .ledger_index
        .ok_or_else(|| "Node data not loaded yet — try again shortly".to_string())?;
    let last_ledger_sequence = u32::try_from(ledger_index)
        .map_err(|_| "Ledger index out of range".to_string())?
        .saturating_add(LAST_LEDGER_OFFSET);

    Ok(LedgerEnv { sequence, fee, last_ledger_sequence })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn order(filled: &str, fc: &str, received: &str, rc: &str, requested: &str, ac: &str) -> Value {
        json!({
            "filled": filled, "filled_currency": fc,
            "received": received, "received_currency": rc,
            "requested": requested, "requested_currency": ac,
            "fill_price": "1.4942",
        })
    }

    
    #[test]
    fn a_fill_is_the_base_amount_at_the_price() {
        let sell = order("1.0000", "XRP", "1.4942", "RLUSD", "1", "XRP");
        let buy = order("1.4942", "RLUSD", "1.0000", "XRP", "1", "XRP");
        assert_eq!(outcome_sentence("success", &sell), "Filled 1 XRP at 1.4942 RLUSD.");
        assert_eq!(outcome_sentence("success", &buy), "Filled 1 XRP at 1.4942 RLUSD.");
        let no_price = json!({"filled": "1", "filled_currency": "XRP", "received": "1.4942", "received_currency": "RLUSD"});
        assert_eq!(outcome_sentence("success", &no_price), "Filled 1 XRP.");
    }

   
    #[test]
    fn a_partial_states_its_shortfall_in_the_unit_typed() {
        let on_xrp = order("0.5", "XRP", "0.7471", "RLUSD", "1", "XRP");
        assert_eq!(
            outcome_sentence("partial", &on_xrp),
            "Filled 0.5 of 1 XRP at 1.4942 RLUSD. The rest did not fill and is gone."
        );
        let on_total = order("0.5", "XRP", "0.7471", "RLUSD", "1.4942", "RLUSD");
        assert_eq!(
            outcome_sentence("partial", &on_total),
            "Received 0.7471 of 1.4942 RLUSD \u{2014} 0.5 XRP at 1.4942 RLUSD. The rest did not fill and is gone."
        );
        assert_eq!(
            outcome_sentence("pending", &on_xrp),
            "Filled 0.5 XRP at 1.4942 RLUSD. The rest is resting on the book."
        );
    }
}
