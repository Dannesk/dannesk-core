

use serde::{Deserialize, Serialize};

use crate::bridge::json_storage;
use crate::channel::PendingTrade;

const FILE: &str = "orders.json";


const CAP: usize = 200;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OrderRecord {
    /// Canonical transaction hash — the join key. Computed from the signed
    /// blob, so it exists before the ledger has seen the order.
    pub hash: String,
    /// The engine pair, pay → receive.
    pub pair: String,
    /// `"pay"` or `"receive"` — which side the user typed, and therefore which
    /// side the bound is a floor or a ceiling on.
    pub anchor: String,
    /// The rate that was signed, receive-per-pay.
    pub bound: f64,
    /// The walked VWAP for this size at quote time. What we expected; `bound`
    /// is what we guaranteed, and the gap between them is the cushion. 
    pub expected_vwap: Option<f64>,
    /// The ledger index the walk was done on. A quote is only as good as the
    /// ledger it was taken from, and drift is measured from here.
    pub ledger_index: u64,
    /// `LastLedgerSequence` on the signed blob — past this the order can no
    /// longer be included, whatever else happens.
    pub last_ledger: Option<u32>,
    pub pay: f64,
    pub receive: f64,
    /// Epoch seconds at submission. Local clock, for ordering only.
    pub submitted_at: u64,
 
    pub status: Option<String>,
    pub filled: Option<f64>,
    pub received: Option<f64>,
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}


pub fn all() -> Vec<OrderRecord> {
    json_storage::read_json::<Vec<OrderRecord>>(FILE).unwrap_or_default()
}

fn save(records: &[OrderRecord]) {
    let _ = json_storage::write_json(FILE, &records.to_vec());
}

pub fn record(hash: &str, quote: &PendingTrade) {
    let mut records = all();
    records.retain(|r| r.hash != hash);
    records.insert(
        0,
        OrderRecord {
            hash: hash.to_string(),
            pair: quote.pair.clone(),
            anchor: quote.anchor.clone(),
            bound: quote.bound,
            expected_vwap: quote.expected_vwap,
            ledger_index: quote.ledger_index,
            last_ledger: quote.last_ledger,
            pay: quote.pay,
            receive: quote.receive,
            submitted_at: now_secs(),
            status: None,
            filled: None,
            received: None,
        },
    );
    records.truncate(CAP);
    save(&records);
}


pub fn settle(hash: &str, status: &str, filled: Option<f64>, received: Option<f64>) {
    let mut records = all();
    let Some(rec) = records.iter_mut().find(|r| r.hash == hash) else {
        return;
    };
    rec.status = Some(status.to_string());
    rec.filled = filled;
    rec.received = received;
    save(&records);
}

#[cfg(test)]
mod tests {
    use super::*;

  
    static FILE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn quote() -> PendingTrade {
        PendingTrade {
            pair: "XRP/RLUSD".into(),
            anchor: "pay".into(),
            bound: 1.3714,
            expected_vwap: Some(1.3731),
            ledger_index: 106_606_128,
            last_ledger: Some(106_606_148),
            sequence: Some(42),
            hash: Some("HASH_A".into()),
            tx_id: Some("uuid-a".into()),
            pay: 1000.0,
            receive: 1371.4,
        }
    }


    #[test]
    fn a_quote_and_its_outcome_are_one_row() {
        let _guard = FILE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _ = json_storage::remove_json(FILE);
        record("HASH_A", &quote());
        settle("HASH_A", "partial", Some(612.0), Some(839.21));

        let rows = all();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].status.as_deref(), Some("partial"));
        assert!((rows[0].bound - 1.3714).abs() < 1e-9, "the promise survives the settle");
        assert!((rows[0].filled.unwrap() - 612.0).abs() < 1e-9);
        let _ = json_storage::remove_json(FILE);
    }

  
    #[test]
    fn an_unknown_hash_does_not_invent_a_row() {
        let _guard = FILE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _ = json_storage::remove_json(FILE);
        settle("HASH_NOBODY", "success", Some(1.0), Some(1.0));
        assert!(all().is_empty());
        let _ = json_storage::remove_json(FILE);
    }

  
    #[test]
    fn history_is_newest_first_and_capped() {
        let _guard = FILE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _ = json_storage::remove_json(FILE);
        for i in 0..(CAP + 5) {
            record(&format!("HASH_{i}"), &quote());
        }
        let rows = all();
        assert_eq!(rows.len(), CAP);
        assert_eq!(rows[0].hash, format!("HASH_{}", CAP + 4));
        let _ = json_storage::remove_json(FILE);
    }
}
