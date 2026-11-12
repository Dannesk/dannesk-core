use crate::bridge::json_storage::remove_json;
use crate::channel::{BtcTransactionState, KeyMode, TransactionState, WSCommand, CHANNEL};

pub fn wipe_wallet_data() {
    let (_, xrp_address, _, _) = CHANNEL.wallet_balance_rx.borrow().clone();
    let (_, btc_address, _, _) = CHANNEL.bitcoin_wallet_rx.borrow().clone();

    crate::gate::delete();

    if let Some(tx) = crate::ws::CRYPTO_COMMANDS_TX.get() {
        if let Some(address) = xrp_address {
            let _ = tx.try_send(WSCommand {
                command: "delete_wallet".to_string(),
                wallet: Some(address),
                ..Default::default()
            });
        }
        if let Some(address) = btc_address {
            let _ = tx.try_send(WSCommand {
                command: "delete_bitcoin_wallet".to_string(),
                wallet: Some(address),
                ..Default::default()
            });
        }
    }

    for file in ["xrp_encrypt.json", "xrp.json", "btc_encrypt.json", "btc.json", "settings.json"] {
        let _ = remove_json(file);
    }

    let _ = CHANNEL.wallet_balance_tx.send((0.0, None, false, KeyMode::Standard));
    let _ = CHANNEL.transactions_tx.send(TransactionState::default());
    CHANNEL.clear_tokens();
    let _ = CHANNEL.bitcoin_wallet_tx.send((0.0, None, false, KeyMode::Standard));
    let _ = CHANNEL.btc_transactions_tx.send(BtcTransactionState::default());
    let _ = CHANNEL.btc_utxos_tx.send((None, Vec::new()));
    let _ = CHANNEL.pending_trade_tx.send(None);
    let _ = CHANNEL.activity_tx.send(None);
}
