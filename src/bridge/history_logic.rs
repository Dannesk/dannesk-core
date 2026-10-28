use crate::channel::{CHANNEL, HistoryList, PageStatus, WSCommand};
use std::time::Duration;

/// page request will wait for maximum of 10s before showing failure.
pub const PAGE_TIMEOUT: Duration = Duration::from_secs(10);

/// Asks the relay for the next page of and fails if timeout.
pub async fn load_more(list: HistoryList) {
    let Some(seq) = request(list) else { return };
    tokio::time::sleep(PAGE_TIMEOUT).await;
    match list {
        HistoryList::BtcTransactions => CHANNEL.btc_transactions_tx.send_modify(|s| s.page.fail(seq)),
        HistoryList::XrpTransactions | HistoryList::XrpOrders => {
            CHANNEL.transactions_tx.send_modify(|s| s.page_mut(list).fail(seq))
        }
    }
}

/// Marks the page fetching and sends the request for the rows.
fn request(list: HistoryList) -> Option<u32> {
    let ws_tx = crate::ws::CRYPTO_COMMANDS_TX.get()?;
    let (wallet, command, offset, seq) = match list {
        HistoryList::BtcTransactions => {
            let wallet = CHANNEL.bitcoin_wallet_rx.borrow().1.clone()?;
            let mut page = None;
            CHANNEL.btc_transactions_tx.send_if_modified(|s| {
                if s.page.status == PageStatus::Fetching {
                    return false;
                }
                page = Some((s.held(), s.page.start()));
                true
            });
            let (offset, seq) = page?;
            (wallet, "get_bitcoin_history", offset, seq)
        }
        HistoryList::XrpTransactions | HistoryList::XrpOrders => {
            let wallet = CHANNEL.wallet_balance_rx.borrow().1.clone()?;
            let mut page = None;
            CHANNEL.transactions_tx.send_if_modified(|s| {
                if s.page(list).status == PageStatus::Fetching {
                    return false;
                }
                page = Some((s.held(list), s.page_mut(list).start()));
                true
            });
            let (offset, seq) = page?;
            (wallet, "get_history", offset, seq)
        }
    };
    let _ = ws_tx.try_send(WSCommand {
        command: command.to_string(),
        wallet: Some(wallet),
        history_kind: Some(list.kind()),
        history_offset: Some(offset),
        ..Default::default()
    });
    Some(seq)
}
