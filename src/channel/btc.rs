use serde::{Deserialize, Serialize};
use std::collections::HashMap;


#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum BitcoinTransactionStatus {
    Success,
    Failed,
    Pending,
    Cancelled,
 
    Dropped,

    Replaced,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BtcRbfInfo {
    pub txid: String,
    /// Measured by the node (`getmempoolentry.vsize`), not estimated.
    pub vsize: u64,
    /// Paid by the original, in satoshis.
    pub fee_sats: u64,
    pub inputs: Vec<BtcRbfInput>,
    pub outputs: Vec<BtcRbfOutput>,
  
    pub incremental_sat_vb: f32,
    pub min_sat_vb: f32,
  
    pub descendants: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BtcRbfInput {
    pub txid: String,
    pub vout: u32,
    pub sats: u64,
 
    pub address: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BtcRbfOutput {
    pub vout: u32,
    pub sats: u64,
    pub address: Option<String>,

    #[serde(default)]
    pub spk: Option<String>,
}



#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct BtcNodeStats {
 
    pub sync: Option<f32>,
    pub peers: Option<u32>,
    pub tip_height: Option<u64>,
 
    pub tip_at: Option<u64>,
  
    pub avg_interval: Option<f32>,
    pub tiers: Option<[f32; 4]>,
  
    pub fee_scale_top: Option<f32>,

    pub incremental_sat_vb: Option<f32>,
  
    pub mempool_txs: Option<u64>,
    pub next_block_txs: Option<u64>,
  
    pub pending_vsize: Option<u64>,
 
    pub bands: Option<[BtcBand; BANDS]>,
  
    pub blocks: [Option<BtcBlock>; BLOCK_TRAIN],

    pub node_height: Option<u64>,
    pub headers: Option<u64>,
   
    pub behind_secs: Option<u64>,

    pub tip_time: Option<u64>,
    pub epoch_start_time: Option<u64>,
}


pub const BLOCK_TRAIN: usize = 13;

pub const BANDS: usize = 14;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BtcBand {
    pub from: f32,
    pub vsize: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BtcBlock {
    pub height: u64,

    pub at: u64,
    pub txs: u64,
    pub weight: u64,
    pub feerate: f32,
}


#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BtcUtxo {
    pub txid: String,
    pub vout: u32,
    pub sats: u64,
 
    pub address: String,

    pub height: u64,
 
    pub spent_by: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct BtcTransactionState {
    pub transactions: HashMap<String, BtcTransactionData>,

    pub page: super::history::HistoryPage,
}

impl BtcTransactionState {

    pub fn held(&self) -> usize {
        self.transactions
            .values()
            .filter(|t| t.status != BitcoinTransactionStatus::Pending)
            .count()
    }

    pub fn apply_reply(&mut self, rows: Vec<BtcTransactionData>, reply: &serde_json::Value, page: bool) {
        for tx in rows {
            self.transactions.insert(tx.txid.clone(), tx);
        }
        self.page.facts(super::history::HistoryList::BtcTransactions, reply);
        if page {
            self.page.done();
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Deserialize)]
pub struct BtcTransactionData {
    pub txid: String,                    
    pub status: BitcoinTransactionStatus, 
  
    pub amount: String,
 
    pub fees: String,
    pub receiver_addresses: Vec<String>, 
   
    pub sender_addresses: Vec<String>,
 
    pub timestamp: String,
   
    #[serde(default)]
    pub confirmed_at: Option<String>,

    #[serde(default)]
    pub dropped_at: Option<String>,
  
    #[serde(default)]
    pub block_height: Option<String>,
   
    #[serde(default)]
    pub replaced_by: Option<String>,

    #[serde(default)]
    pub inputs: Vec<BtcRbfInput>,
    #[serde(default)]
    pub outputs: Vec<BtcRbfOutput>,
    #[serde(default)]
    pub vsize: Option<u64>,
}


pub type BtcSendDispatches = u64;
