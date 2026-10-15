

use crate::channel::{CHANNEL, WSCommand};
use crate::ws::commands::bitcoin_submit_transaction::{
    advance_log, dispatched, fail_log, stash_in_flight,
};
use crate::ws::commands::{
    bitcoin_auth, bitcoin_payment, bitcoin_transaction_sender, bitcoin_validation,
};

pub async fn execute(bitcoin_current_wallet: String, mut cmd: WSCommand) -> Result<(), String> {
    let (tx_type, wallet, _) = bitcoin_validation::validate_inputs(&cmd, &bitcoin_current_wallet)
        .map_err(|e| {
            fail_log(&e);
            e
        })?;
    let fee: u64 = match cmd.fee.as_deref().and_then(|f| f.parse().ok()) {
        Some(f) => f,
        None => {
            fail_log("Invalid transaction fee");
            return Err("Error: Invalid transaction fee format".to_string());
        }
    };
    let Some(old_txid) = cmd.replaces.clone() else {
        fail_log("No transaction to replace");
        return Err("Error: no transaction to replace".to_string());
    };
  
    let Some(info) = bitcoin_payment::rbf_info_for(&old_txid) else {
        fail_log("Fee-bump details not in view");
        return Err("no details for this transaction".to_string());
    };

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
        Ok(Err(e)) | Err(e) => {
            fail_log("Authentication failed");
            return Err(e);
        }
    };

    advance_log("auth", "build");

    let ours: Vec<String> = crate::wallet::btc_address_records()
        .into_iter()
        .map(|r| r.address)
        .collect();
    let spare = bitcoin_payment::spare_coin(&wallet, &info);
    let plan = match bitcoin_payment::plan_replacement(&info, &ours, spare.as_ref(), fee) {
        Ok(p) => p,
        Err(e) => {
            fail_log("Failed to build replacement");
            return Err(e);
        }
    };

    let change_address = if plan.change_sats.is_some() && plan.change_spk.is_none() {
        crate::bridge::btc_receive_rotation::ensure_change_address()
            .unwrap_or_else(|| wallet_obj.address.clone())
    } else {
        String::new()
    };
    let tx_hex = match bitcoin_payment::construct_replacement(&wallet_obj, &plan, &change_address) {
        Ok(h) => h,
        Err(e) => {
            fail_log("Failed to build replacement");
            return Err(e);
        }
    };

    advance_log("build", "broadcast");

    if !*CHANNEL.btc_ws_status_rx.borrow() {
        fail_log("No connection to the network — nothing was sent.");
        return Err("Error: no connection to the network".to_string());
    }

    stash_in_flight(&wallet, &tx_hex, Some(&old_txid));
    dispatched();

    if let Err(e) =
     
        bitcoin_transaction_sender::send_transaction(&wallet, &tx_type, tx_hex, Some(&old_txid), None).await
    {
        crate::ws::commands::bitcoin_submit_transaction::discard_in_flight(None);
        fail_log(&e);
        return Err(e);
    }

    advance_log("broadcast", "confirm");
    Ok(())
}
