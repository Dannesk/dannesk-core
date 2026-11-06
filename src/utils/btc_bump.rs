use crate::channel::{BtcRbfInfo, CHANNEL};
use crate::utils::btc_send::printed_rate;
use crate::ws::commands::bitcoin_payment::{min_replacement_fee, quote_replacement, rbf_info_for, replacement, spare_coins, tier_sats_at, Utxo};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BtcBumpDraft {
    pub txid: String,
    pub custom: Option<String>,
}


#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct BumpFee {
    pub rate: f32,
    pub fee: u64,
    pub vsize: u64,
    pub extra: u64,
    pub adds_coin: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BtcBumpQuote {
    pub draft: BtcBumpDraft,
    pub now: BumpFee,
    pub min_rate: f32,
    pub tiers: [Option<BumpFee>; 4],
    pub custom: Option<BumpFee>,
}

fn printed_below(rate: f32) -> f32 {
    let r = rate as f64;
    let step = if r <= 10.0 { 0.01 } else if r <= 100.0 { 0.1 } else { 1.0 };
    if r - step <= 0.0 { 0.0 } else { printed_rate(r - step) }
}


fn lowest_rate(fee: u64, vsize: u64) -> f32 {
    let mut rate = printed_rate(fee as f64 / vsize.max(1) as f64);
    for _ in 0..2 {
        let below = printed_below(rate);
        if below > 0.0 && tier_sats_at(below, vsize) >= fee {
            rate = below;
        } else {
            break;
        }
    }
    rate
}

/// The lowest rate the network is willing to take for replacement.
fn min_rate(info: &BtcRbfInfo, own: &str, spares: &[Utxo]) -> f32 {
    let mut vsize = info.vsize;
    let mut floor = min_replacement_fee(info, vsize);
    for _ in 0..3 {
        match replacement(info, own, spares, floor, Some(info.min_sat_vb)) {
            Ok(plan) if plan.vsize != vsize => {
                vsize = plan.vsize;
                floor = min_replacement_fee(info, vsize);
            }
            _ => break,
        }
    }
    lowest_rate(floor, vsize)
}

pub fn price(draft: &BtcBumpDraft, info: &BtcRbfInfo, own: &str, spares: &[Utxo], tiers: [f32; 4]) -> BtcBumpQuote {
    let bump = |rate: f32, fee: u64, vsize: u64, adds_coin: bool| BumpFee {
        rate,
        fee,
        vsize,
        extra: fee.saturating_sub(info.fee_sats),
        adds_coin,
    };
    let at = |rate: f32, floor: bool| match quote_replacement(info, own, spares, rate) {
        Ok(plan) => Some(bump(rate, plan.fee, plan.vsize, plan.added_input)),
        Err(_) => {
            let fee = tier_sats_at(rate, info.vsize);
            (!floor || fee >= min_replacement_fee(info, info.vsize)).then(|| bump(rate, fee, info.vsize, false))
        }
    };
    let custom = draft
        .custom
        .as_deref()
        .and_then(|r| r.trim().parse::<f64>().ok())
        .filter(|r| r.is_finite() && *r > 0.0)
        .map(printed_rate);
    BtcBumpQuote {
        draft: draft.clone(),
        now: BumpFee {
            rate: (info.fee_sats as f64 / info.vsize.max(1) as f64) as f32,
            fee: info.fee_sats,
            vsize: info.vsize,
            extra: 0,
            adds_coin: false,
        },
        min_rate: min_rate(info, own, spares),
        tiers: tiers.map(|t| at(t, true)),
        custom: custom.and_then(|r| at(r, false)),
    }
}

pub fn compute(draft: &BtcBumpDraft) -> Option<BtcBumpQuote> {
    let own = CHANNEL.bitcoin_wallet_rx.borrow().1.clone()?;
    let info = rbf_info_for(&draft.txid).ok()?;
    let tiers = CHANNEL.btc_node_rx.borrow().tiers?;
    let spares = spare_coins(&own, &info);
    Some(price(draft, &info, &own, &spares, tiers))
}

pub async fn quote_task() {
    let mut draft_rx = CHANNEL.btc_bump_draft_rx.clone();
    let mut wallet_rx = CHANNEL.bitcoin_wallet_rx.clone();
    let mut utxos_rx = CHANNEL.btc_utxos_rx.clone();
    let mut txs_rx = CHANNEL.btc_transactions_rx.clone();
    let mut node_rx = CHANNEL.btc_node_rx.clone();
    loop {
        tokio::select! {
            r = draft_rx.changed() => { if r.is_err() { return; } }
            r = wallet_rx.changed() => { if r.is_err() { return; } }
            r = utxos_rx.changed() => { if r.is_err() { return; } }
            r = txs_rx.changed() => { if r.is_err() { return; } }
            r = node_rx.changed() => { if r.is_err() { return; } }
        }
        let draft = draft_rx.borrow_and_update().clone();
        wallet_rx.mark_unchanged();
        utxos_rx.mark_unchanged();
        txs_rx.mark_unchanged();
        node_rx.mark_unchanged();
        let quote = draft.as_ref().and_then(compute);
        CHANNEL.btc_bump_quote_tx.send_if_modified(|q| {
            if *q == quote {
                false
            } else {
                *q = quote;
                true
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel::{BtcRbfInput, BtcRbfOutput};

    const OWN: &str = "bc1qcr8te4kr609gcawutmrza0j4xv80jy8z306fyu";
    const THEM_SPK: &str = "0014aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const OWN_SPK: &str = "0014bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const TODAY: [f32; 4] = [0.10, 0.52, 1.20, 3.10];

    fn sent() -> BtcRbfInfo {
        BtcRbfInfo {
            txid: "aa".repeat(32),
            vsize: 141,
            fee_sats: 48,
            inputs: vec![BtcRbfInput { txid: "11".repeat(32), vout: 0, sats: 300_000, address: Some(OWN.into()) }],
            outputs: vec![
                BtcRbfOutput { vout: 0, sats: 250_000, address: Some("them".into()), spk: Some(THEM_SPK.into()) },
                BtcRbfOutput { vout: 1, sats: 49_952, address: Some(OWN.into()), spk: Some(OWN_SPK.into()) },
            ],
            incremental_sat_vb: 0.10,
            min_sat_vb: 0.10,
            descendants: 0,
        }
    }

    fn draft(custom: Option<&str>) -> BtcBumpDraft {
        BtcBumpDraft { txid: "aa".repeat(32), custom: custom.map(String::from) }
    }

    fn fees(q: &BtcBumpQuote) -> [Option<u64>; 4] {
        q.tiers.map(|t| t.map(|b| b.fee))
    }

    #[test]
    fn the_minimum_is_the_old_rate_plus_the_incremental_fee() {
        let q = price(&draft(None), &sent(), OWN, &[], TODAY);
        assert_eq!(q.min_rate, 0.44);
        assert_eq!(tier_sats_at(q.min_rate, 141), min_replacement_fee(&sent(), 141));
        assert_eq!((q.now.fee, q.now.vsize), (48, 141));
        assert!((q.now.rate - 0.34).abs() < 0.001);
    }

    #[test]
    fn only_tiers_at_or_above_the_minimum_are_offered() {
        let q = price(&draft(None), &sent(), OWN, &[], TODAY);
        assert_eq!(fees(&q), [None, Some(74), Some(170), Some(438)]);
        assert_eq!(q.tiers[1].map(|b| (b.extra, b.vsize, b.adds_coin)), Some((26, 141, false)));
        let at_floor = price(&draft(None), &sent(), OWN, &[], [0.10, 0.44, 0.45, 3.10]);
        assert_eq!(fees(&at_floor), [None, Some(63), Some(64), Some(438)], "a tier exactly at the minimum shows");
    }

    #[test]
    fn a_custom_rate_is_priced_even_under_the_minimum() {
        let q = price(&draft(Some("0.4")), &sent(), OWN, &[], TODAY);
        let custom = q.custom.unwrap();
        assert_eq!((custom.rate, custom.fee), (0.40, 57));
        assert!(custom.rate < q.min_rate);
        let q = price(&draft(Some("0.44")), &sent(), OWN, &[], TODAY);
        assert_eq!(q.custom.map(|c| (c.fee, c.rate >= q.min_rate)), Some((63, true)));
        for bad in ["", "abc", "0", "-1", "inf"] {
            assert_eq!(price(&draft(Some(bad)), &sent(), OWN, &[], TODAY).custom, None, "{bad:?} was priced");
        }
    }

    #[test]
    fn a_bump_the_leftover_cannot_pay_adds_a_coin() {
        let mut small = sent();
        small.outputs[1].sats = 400;
        small.inputs[0].sats = 250_448;
        let spare = Utxo { txid: "22".repeat(32), vout: 1, amount: 50_000, address: OWN.into() };
        let q = price(&draft(None), &small, OWN, std::slice::from_ref(&spare), [0.10, 0.52, 1.20, 5.0]);
        let high = q.tiers[3].unwrap();
        assert!(high.adds_coin);
        assert_eq!(high.vsize, 209);
        assert_eq!(high.fee, tier_sats_at(5.0, 209));
        assert_eq!(q.tiers[1].map(|b| (b.fee, b.adds_coin)), Some((74, false)));
    }

    #[test]
    fn a_send_that_cannot_be_bumped_is_still_priced_for_signing_to_refuse() {
        let mut later = sent();
        later.descendants = 1;
        let q = price(&draft(None), &later, OWN, &[], TODAY);
        assert_eq!(fees(&q), [None, Some(74), Some(170), Some(438)]);
        let mut max = sent();
        max.outputs.pop();
        max.fee_sats = 50_000;
        max.inputs[0].sats = 300_000;
        let q = price(&draft(None), &max, OWN, &[], [0.10, 0.52, 400.0, 500.0]);
        assert_eq!(q.tiers[2].map(|b| (b.fee, b.adds_coin)), Some((56_400, false)));
    }
}
