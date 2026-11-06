use crate::channel::{BtcRbfInfo, CHANNEL};
use crate::utils::btc_send::printed_rate;
use crate::ws::commands::bitcoin_payment::{min_replacement_fee, price_replacement, rbf_info_for, spare_coins, BumpPrice, Utxo};
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

/// The printed rates.
fn printed_at(i: u32) -> f32 {
    let v = if i < 1000 {
        i as f64 / 100.0
    } else if i < 1900 {
        (i - 900) as f64 / 10.0
    } else {
        (i - 1800) as f64
    };
    printed_rate(v)
}

fn index_of(rate: f32) -> u32 {
    let r = (rate as f64 * 1e6).round() / 1e6;
    if r < 10.0 {
        (r * 100.0).round() as u32
    } else if r < 100.0 {
        (r * 10.0).round() as u32 + 900
    } else {
        r.round() as u32 + 1800
    }
}

/// The highest rate searched
const TOP: u32 = 101_800;

/// The lowest printed rate whose replacement the network will take, which we find by halving since a higher rate never pays less.
fn min_rate(info: &BtcRbfInfo, own: &str, spares: &[Utxo]) -> f32 {
    let clears = |i: u32| price_replacement(info, own, spares, printed_at(i)).clears(info);
    let floor = min_replacement_fee(info, info.vsize);
    let mut hi = index_of(printed_rate(floor as f64 / info.vsize.max(1) as f64)).max(1);
    while !clears(hi) && hi < TOP {
        hi = (hi * 2).min(TOP);
    }
    let mut lo = 0;
    while hi - lo > 1 {
        let mid = lo + (hi - lo) / 2;
        if clears(mid) {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    printed_at(hi)
}

pub fn price(draft: &BtcBumpDraft, info: &BtcRbfInfo, own: &str, spares: &[Utxo], tiers: [f32; 4]) -> BtcBumpQuote {
    let bump = |rate: f32, p: &BumpPrice| BumpFee {
        rate,
        fee: p.fee(),
        vsize: p.vsize(),
        extra: p.fee().saturating_sub(info.fee_sats),
        adds_coin: match p {
            BumpPrice::Built(plan) => plan.added_input,
            BumpPrice::Unbuilt { adds_coin, .. } => *adds_coin,
        },
    };
    let at = |rate: f32, floor: bool| {
        let p = price_replacement(info, own, spares, rate);
        (!floor || p.clears(info)).then(|| bump(rate, &p))
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
    use crate::ws::commands::bitcoin_payment::{replacement_at, tier_sats_at};

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

    fn spent(sats: u64, fee: u64, leftover: u64, vsize: u64, min: f32) -> BtcRbfInfo {
        let mut info = sent();
        info.inputs[0].sats = sats;
        info.outputs[0].sats = sats - fee - leftover;
        if leftover == 0 {
            info.outputs.pop();
        } else {
            info.outputs[1].sats = leftover;
        }
        info.fee_sats = fee;
        info.vsize = vsize;
        info.min_sat_vb = min;
        info
    }

    fn coin(txid: &str, amount: u64) -> Utxo {
        Utxo { txid: txid.repeat(32), vout: 1, amount, address: OWN.into() }
    }

    #[test]
    fn a_bump_that_needs_a_coin_is_priced_at_the_size_it_signs() {
        let max = spent(51_100, 1_100, 0, 110, 1.0);
        let spares = [coin("22", 50_000)];
        let q = price(&draft(None), &max, OWN, &spares, [1.0, 2.0, 5.0, 12.0]);
        assert_eq!(q.min_rate, 10.1);
        let high = q.tiers[3].unwrap();
        assert_eq!((high.fee, high.vsize, high.adds_coin), (2_508, 209, true));
        let signed = replacement_at(&max, OWN, &spares, high.fee, high.vsize).unwrap();
        assert_eq!((signed.fee, signed.vsize), (2_508, 209));
        let q = price(&draft(Some("10.1")), &max, OWN, &spares, [1.0, 2.0, 5.0, 12.0]);
        let custom = q.custom.unwrap();
        assert_eq!((custom.fee, custom.vsize, custom.adds_coin), (2_111, 209, true));
        assert!(replacement_at(&max, OWN, &spares, custom.fee, custom.vsize).is_ok());
    }

    #[test]
    fn a_fee_the_extra_coin_cannot_reach_is_left_for_signing_to_refuse() {
        let max = spent(50_110, 110, 0, 110, 1.0);
        let spares = [coin("22", 900)];
        let q = price(&draft(None), &max, OWN, &spares, [1.0, 2.0, 3.0, 5.0]);
        let high = q.tiers[3].unwrap();
        assert_eq!((high.fee, high.vsize, high.adds_coin), (1_045, 209, true), "never 550 sat at 110 vB, which would sign at half the rate");
        assert!(replacement_at(&max, OWN, &spares, high.fee, high.vsize).is_err());
    }

    #[test]
    fn the_minimum_steps_down_to_the_last_rate_that_clears() {
        let q = price(&draft(None), &spent(300_000, 45, 49_955, 141, 0.10), OWN, &[], TODAY);
        assert_eq!(q.min_rate, 0.42);
    }

    #[test]
    fn a_leftover_paid_to_the_fee_lowers_the_minimum() {
        let info = spent(251_105, 705, 400, 141, 5.0);
        let q = price(&draft(Some("5")), &info, OWN, &[], [5.0, 5.5, 6.0, 7.0]);
        assert_eq!(q.min_rate, 4.37);
        let custom = q.custom.unwrap();
        assert_eq!((custom.fee, custom.vsize), (1_105, 110));
        assert!(custom.rate >= q.min_rate);
        assert_eq!(q.tiers[1].map(|b| b.fee), Some(1_105));
    }

    #[test]
    fn the_minimum_agrees_with_every_rate_the_network_takes() {
        let cases: Vec<(BtcRbfInfo, Vec<Utxo>)> = vec![
            (sent(), vec![]),
            (spent(300_000, 45, 49_955, 141, 0.10), vec![]),
            (spent(251_105, 705, 400, 141, 5.0), vec![]),
            (spent(51_100, 1_100, 0, 110, 1.0), vec![coin("22", 50_000)]),
            (spent(50_110, 110, 0, 110, 1.0), vec![coin("22", 900)]),
            (spent(251_105, 705, 400, 141, 5.0), vec![coin("21", 300), coin("22", 2_000), coin("23", 90_000)]),
            (BtcRbfInfo { descendants: 1, ..sent() }, vec![]),
        ];
        for (info, spares) in cases {
            let min = min_rate(&info, OWN, &spares);
            for i in 1..2_100 {
                let rate = printed_at(i);
                let clears = price_replacement(&info, OWN, &spares, rate).clears(&info);
                assert_eq!(clears, rate >= min, "fee {} at {} vB, rate {rate}, minimum {min}", info.fee_sats, info.vsize);
            }
        }
    }

    #[test]
    fn the_printed_rates_index_both_ways() {
        for i in 1..2_500 {
            assert_eq!(index_of(printed_at(i)), i, "{}", printed_at(i));
        }
        assert_eq!([printed_at(999), printed_at(1000), printed_at(1899), printed_at(1900)], [9.99, 10.0, 99.9, 100.0]);
    }
}
