use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::history::{HistoryList, HistoryPage};


#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct XrpAccount {
    pub sequence: Option<u32>,
    pub owner_count: Option<u32>,
}


#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct XrpNodeStats {
    pub ledger_index: Option<u64>,
    pub fee_base: Option<u64>,
    pub open_ledger_fee: Option<u64>,
    pub reserve_base: Option<u64>,
    pub reserve_inc: Option<u64>,
    pub txn_count: Option<u64>,
    pub peers: Option<u64>,
    pub synced: Option<bool>,
    pub state: Option<String>,
}


#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum TxTab {
    Open,
    #[default]
    Settled,
}


#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum TransactionStatus {
    Success,
 
    Partial,
    
    Killed,
    Failed,
    Pending,
    Cancelled,
}


#[derive(Debug, Clone, PartialEq)]
pub struct PendingTrade {
    pub pair: String,
  
    pub anchor: String,
    pub bound: f64,
    
    pub expected_vwap: Option<f64>,
    pub ledger_index: u64,
  
    pub last_ledger: Option<u32>,
 
    pub hash: Option<String>,
   
    pub sequence: Option<u32>,
  
    pub tx_id: Option<String>,
    pub pay: f64,
    pub receive: f64,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct TransactionState {
    pub transactions: HashMap<String, TransactionData>,
    pub tx_page: HistoryPage,
 
    pub orders_page: HistoryPage,
}

impl TransactionState {

    pub fn is_order(t: &TransactionData) -> bool {
        matches!(t.order_type.as_str(), "offercreate" | "offer_create")
    }

    
    pub fn held(&self, list: HistoryList) -> usize {
        self.transactions
            .values()
            .filter(|t| match list {
                HistoryList::XrpTransactions => !Self::is_order(t),
                HistoryList::XrpOrders => Self::is_order(t) && t.status != TransactionStatus::Pending,
                HistoryList::BtcTransactions => false,
            })
            .count()
    }

    pub fn page(&self, list: HistoryList) -> &HistoryPage {
        match list {
            HistoryList::XrpOrders => &self.orders_page,
            _ => &self.tx_page,
        }
    }

    pub fn page_mut(&mut self, list: HistoryList) -> &mut HistoryPage {
        match list {
            HistoryList::XrpOrders => &mut self.orders_page,
            _ => &mut self.tx_page,
        }
    }

    pub fn apply_reply(&mut self, rows: Vec<TransactionData>, reply: &serde_json::Value, page: Option<HistoryList>) {
        for tx in rows {
            self.transactions.insert(tx.tx_id.clone(), tx);
        }
        self.tx_page.facts(HistoryList::XrpTransactions, reply);
        self.orders_page.facts(HistoryList::XrpOrders, reply);
        if let Some(list) = page {
            self.page_mut(list).done();
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Deserialize)]
pub struct TransactionData {
    pub tx_id: String,
    pub status: TransactionStatus,
    /// The node's own result code, `tesSUCCESS` or the `tec` code.
    pub result: Option<String>,
    pub execution_price: String,
    pub order_type: String,
    pub timestamp: String,
 
    pub amount: String,
    pub currency: String,
    pub fee: String,
    pub flags: Option<String>,
    pub receiver: String,
    pub sender: String,
    pub sequence: Option<u32>,
    
    pub destination_tag: Option<u32>,
  
    pub pay_amount: Option<String>,
    pub pay_currency: Option<String>,

    pub filled: Option<String>,
    pub filled_currency: Option<String>,
    pub received: Option<String>,
    pub received_currency: Option<String>,

    pub remaining: Option<String>,
    pub remaining_currency: Option<String>,
    pub coverage: Option<f64>,
  
    pub fill_price: Option<String>,
}




#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Trade {
    pub step: u8,
    pub base_asset: Option<String>,
    pub quote_asset: Option<String>,
    pub amount: Option<String>,
    pub limit_price: Option<String>,
    pub fee_percentage: f64,
    pub flags: Option<Vec<String>>,
    pub error: Option<String>,
    pub asset: String,
}

