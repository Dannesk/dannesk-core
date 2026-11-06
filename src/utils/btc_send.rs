use crate::btc_script_type::{self, BtcScriptType};
use crate::channel::CHANNEL;
use crate::ws::commands::bitcoin_payment::{change_floor, dust_size, eligible_utxos, prefix_vsizes, tier_sats_at, vsize_for, Utxo, DUST_RELAY_FEE};
use dannesk_btc_codec::address::{Address, Kind};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BtcSendDraft {
    pub recipient: String,
    pub amount: String,
    pub custom: Option<String>,
    pub max: bool,
}

/// fee in sats, the vsize it priced and the max.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct FeeQuote {
    pub rate: f32,
    pub fee: u64,
    pub vsize: u64,
    pub max: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BtcSendQuote {
    pub draft: BtcSendDraft,
    pub spendable: Option<u64>,
    pub dust: Option<u64>,
    pub tiers: Option<[FeeQuote; 4]>,
    pub custom: Option<FeeQuote>,
}

pub fn dust_limit(address: &Address) -> u64 {
    let witness = matches!(address.kind(), Kind::P2wpkh | Kind::P2wsh | Kind::P2tr | Kind::OtherWitness);
    tier_sats_at(DUST_RELAY_FEE, dust_size(address.script_pubkey().len(), witness))
}

fn amount_sats(amount: &str) -> u64 {
    amount
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|v| *v > 0.0)
        .map_or(0, |v| (v * 100_000_000.0).round() as u64)
}

/// rounded up
pub fn printed_rate(v: f64) -> f32 {
    let up = |scale: f64| ((v * scale * 1e6).round() / 1e6).ceil() / scale;
    let r = up(100.0);
    if r < 10.0 {
        return r as f32;
    }
    let r = up(10.0);
    if r < 100.0 {
        return r as f32;
    }
    up(1.0) as f32
}

fn priced(rate: f32, coins: &[Utxo], kinds: &[BtcScriptType], own: BtcScriptType, amount: u64, recipient_len: usize, floor: u64) -> (u64, u64) {
    let first = |n: usize| -> Vec<BtcScriptType> {
        if kinds.is_empty() { vec![own; n] } else { kinds[..n.min(kinds.len())].to_vec() }
    };
    let at = |n: usize, outs: &[usize]| {
        let vsize = vsize_for(&first(n), outs);
        (tier_sats_at(rate, vsize), vsize)
    };
    let cover = |outs: &[usize]| {
        let mut sum = 0u64;
        coins.iter().zip(prefix_vsizes(kinds, outs)).enumerate().find_map(|(i, (coin, vsize))| {
            sum += coin.amount;
            let fee = tier_sats_at(rate, vsize);
            (sum >= amount.saturating_add(fee)).then_some((i + 1, sum, fee, vsize))
        })
    };
    let two_out = [recipient_len, own.spk_len()];
    let one_out = [recipient_len];
    let unaffordable = || at(coins.len().max(1), &two_out);

    let change = cover(&two_out);
    if amount == 0 {
        return change.map_or_else(unaffordable, |(_, _, fee, vsize)| (fee, vsize));
    }
    match (change, cover(&one_out)) {
        // Fewer coins without change always cost less than more coins with it.
        (Some((n, held, fee, _)), Some((m, sum, _, size))) if m < n || held - amount - fee < floor => (sum - amount, size),
        (Some((_, _, fee, vsize)), _) => (fee, vsize),
        (None, Some((_, sum, _, size))) => (sum - amount, size),
        (None, None) => unaffordable(),
    }
}

pub fn price(draft: &BtcSendDraft, own: BtcScriptType, coins: Option<&[Utxo]>, tiers: Option<[f32; 4]>) -> BtcSendQuote {
    let held = coins.unwrap_or(&[]);
    let kinds: Vec<BtcScriptType> = held.iter().map(|c| BtcScriptType::of_address(&c.address).unwrap_or(own)).collect();
    let spendable = coins.map(|c| c.iter().map(|u| u.amount).sum::<u64>());
    let recipient = Address::parse(draft.recipient.trim()).ok();
    let recipient_len = recipient.as_ref().map_or(own.spk_len(), |a| a.script_pubkey().len());
    let amount = amount_sats(&draft.amount);
    let sweep = vsize_for(&kinds, &[recipient_len]);
    let floor = change_floor(own, tiers.map(|t| t[0]));
    let quote = |rate: f32| {
        let swept = tier_sats_at(rate, sweep);
        let (fee, vsize) = if draft.max && !held.is_empty() {
            (swept, sweep)
        } else {
            priced(rate, held, &kinds, own, amount, recipient_len, floor)
        };
        FeeQuote { rate, fee, vsize, max: spendable.unwrap_or(0).saturating_sub(swept) }
    };
    let custom = draft
        .custom
        .as_deref()
        .and_then(|r| r.trim().parse::<f64>().ok())
        .filter(|r| r.is_finite() && *r > 0.0)
        .map(printed_rate);
    BtcSendQuote {
        draft: draft.clone(),
        spendable,
        dust: recipient.as_ref().map(dust_limit),
        tiers: tiers.map(|t| t.map(&quote)),
        custom: custom.map(&quote),
    }
}

pub fn compute(draft: &BtcSendDraft) -> BtcSendQuote {
    let wallet = CHANNEL.bitcoin_wallet_rx.borrow().1.clone();
    let coins = wallet.as_deref().and_then(|w| eligible_utxos(w).ok());
    let own = wallet
        .as_deref()
        .and_then(BtcScriptType::of_address)
        .unwrap_or_else(btc_script_type::stored);
    let tiers = CHANNEL.btc_node_rx.borrow().tiers;
    price(draft, own, coins.as_deref(), tiers)
}

pub async fn quote_task() {
    let mut draft_rx = CHANNEL.btc_send_draft_rx.clone();
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
        let quote = draft.as_ref().map(compute);
        CHANNEL.btc_send_quote_tx.send_if_modified(|q| {
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

    const P2PKH: &str = "1BvBMSEYstWetqTFn5Au4m4GFg7xJaNVN2";
    const P2SH: &str = "3J98t1WpEZ73CNmQviecrnyiWrnqRhWNLy";
    const P2WPKH: &str = "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4";
    const P2WSH: &str = "bc1qrp33g0q5c5txsp9arysrx4k6zdkfs4nce4xj0gdcccefvpysxf3qccfmv3";
    const P2TR: &str = "bc1p0xlxvlhemja6c4dqv22uapctqupfhlxm9h8z3k2e72q4k9hcz7vqzk5jj0";
    const OWN: &str = "bc1qcr8te4kr609gcawutmrza0j4xv80jy8z306fyu";
    const AUGUST: [f32; 4] = [0.10, 0.13, 0.34, 1.09];

    fn coins(sats: &[u64]) -> Vec<Utxo> {
        coins_at(OWN, sats)
    }

    fn coins_at(address: &str, sats: &[u64]) -> Vec<Utxo> {
        sats.iter()
            .enumerate()
            .map(|(i, s)| Utxo { txid: format!("{:02x}", 0x11 + i).repeat(32), vout: 0, amount: *s, address: address.to_string() })
            .collect()
    }

    fn draft(recipient: &str, amount: &str, custom: Option<&str>) -> BtcSendDraft {
        BtcSendDraft { recipient: recipient.to_string(), amount: amount.to_string(), custom: custom.map(String::from), max: false }
    }

    fn fees(q: &BtcSendQuote) -> [u64; 4] {
        q.tiers.unwrap().map(|t| t.fee)
    }

    fn sizes(q: &BtcSendQuote) -> [u64; 4] {
        q.tiers.unwrap().map(|t| t.vsize)
    }

    #[test]
    fn a_rate_rounds_up_to_the_places_it_prints() {
        let cases = [(0.1, 0.10), (0.1013, 0.11), (0.13, 0.13), (2.004, 2.01), (9.994, 10.0), (18.44, 18.5), (99.96, 100.0), (312.47, 313.0)];
        for (raw, shown) in cases {
            assert_eq!(printed_rate(raw), shown as f32, "{raw}");
        }
    }

    #[test]
    fn rates_that_print_alike_cost_alike() {
        let set = coins(&[5_000]);
        let tiers = [0.1, 0.1013, 0.1036, 2.004].map(printed_rate);
        let q = price(&BtcSendDraft::default(), BtcScriptType::NativeSegwit, Some(&set), Some(tiers));
        assert_eq!(fees(&q), [15, 16, 16, 284]);
        assert_eq!(sizes(&q), [141; 4]);
    }

    #[test]
    fn under_max_every_tier_costs_its_own_sweep() {
        let set = coins(&[5_000]);
        let tiers = Some([0.10, 0.11, 0.11, 2.01]);
        let max = BtcSendDraft { max: true, ..draft(P2WPKH, "0.00004987", None) };
        let q = price(&max, BtcScriptType::NativeSegwit, Some(&set), tiers);
        assert_eq!(fees(&q), [11, 13, 13, 222]);
        assert_eq!(sizes(&q), [110; 4]);
        assert_eq!(q.tiers.unwrap().map(|t| t.max + t.fee), [5_000; 4]);

        let typed = draft(P2WPKH, "0.00004987", None);
        let q = price(&typed, BtcScriptType::NativeSegwit, Some(&set), tiers);
        assert_eq!(fees(&q), [13, 13, 13, 284]);
        assert_eq!(sizes(&q), [110, 110, 110, 141]);
    }

    #[test]
    fn change_under_the_floor_goes_to_the_fee() {
        let set = coins(&[3_600]);
        let q = price(&draft(P2WPKH, "0.00003100", None), BtcScriptType::NativeSegwit, Some(&set), Some(AUGUST));
        assert_eq!(fees(&q), [15, 19, 48, 154]);
        assert_eq!(sizes(&q), [141; 4]);
        let q = price(&draft(P2WPKH, "0.00003400", None), BtcScriptType::NativeSegwit, Some(&set), Some(AUGUST));
        assert_eq!(fees(&q), [200; 4]);
        assert_eq!(sizes(&q), [110; 4]);
    }

    #[test]
    fn the_change_floor_is_cores() {
        use crate::ws::commands::bitcoin_payment::change_floor;
        assert_eq!(change_floor(BtcScriptType::NativeSegwit, None), 294);
        assert_eq!(change_floor(BtcScriptType::NativeSegwit, Some(0.10)), 294);
        assert_eq!(change_floor(BtcScriptType::NativeSegwit, Some(5.0)), 490);
        assert_eq!(change_floor(BtcScriptType::NativeSegwit, Some(40.0)), 980);
        assert_eq!(change_floor(BtcScriptType::Taproot, None), 330);
        assert_eq!(change_floor(BtcScriptType::NestedSegwit, None), 540);
        assert_eq!(change_floor(BtcScriptType::Legacy, None), 546);
    }

    #[test]
    fn the_floor_rises_with_the_slowest_rate() {
        let set = coins(&[10_000]);
        let q = price(&draft(P2WPKH, "0.00007000", None), BtcScriptType::NativeSegwit, Some(&set), Some([12.0; 4]));
        assert_eq!((fees(&q)[0], sizes(&q)[0]), (1_692, 141));
        let q = price(&draft(P2WPKH, "0.00007500", None), BtcScriptType::NativeSegwit, Some(&set), Some([12.0; 4]));
        assert_eq!((fees(&q)[0], sizes(&q)[0]), (2_500, 110));
        let q = price(&draft(P2WPKH, "0.00007500", None), BtcScriptType::NativeSegwit, Some(&set), Some([1.0; 4]));
        assert_eq!((fees(&q)[0], sizes(&q)[0]), (141, 141));
    }

    #[test]
    fn many_small_coins_ahead_of_a_big_one_are_walked() {
        let mut sats = vec![300; 79];
        sats.push(1_000_000);
        let set = coins(&sats);
        let q = price(&draft(P2WPKH, "0.00002000", None), BtcScriptType::NativeSegwit, Some(&set), Some([4.0; 4]));
        assert_eq!(fees(&q), [22_132; 4]);
        assert_eq!(sizes(&q), [5_533; 4]);
    }

    #[test]
    fn fewer_coins_without_change_beat_more_coins_with_it() {
        let set = coins(&[10_000, 400]);
        let q = price(&draft(P2WPKH, "0.00009865", Some("1.2")), BtcScriptType::NativeSegwit, Some(&set), Some([1.0, 2.0, 3.0, 5.0]));
        assert_eq!(fees(&q), [135, 535, 535, 1_045]);
        assert_eq!(sizes(&q), [110, 178, 178, 209]);
        assert_eq!(q.custom.map(|c| (c.fee, c.vsize)), Some((135, 110)));
    }

    #[test]
    fn a_higher_rate_never_costs_less() {
        let ladder = [0.10, 0.50, 1.0, 1.2, 2.0, 3.0, 5.0, 12.0, 40.0];
        let sets: [&[u64]; 4] = [&[10_000, 400], &[3_600], &[30_000, 20_000, 5_000], &[300, 300, 300, 50_000]];
        let wallets = [(BtcScriptType::NativeSegwit, OWN), (BtcScriptType::Taproot, P2TR), (BtcScriptType::NestedSegwit, P2SH), (BtcScriptType::Legacy, P2PKH)];
        for (own, address) in wallets {
            for sats in sets {
                let held = coins_at(address, sats);
                let total: u64 = sats.iter().sum();
                for amount in (500..total).step_by(97) {
                    let typed = format!("{:.8}", amount as f64 / 1e8);
                    let fees = ladder.map(|r| {
                        let q = price(&draft(P2WPKH, &typed, Some(&r.to_string())), own, Some(&held), Some([0.10; 4]));
                        q.custom.unwrap().fee
                    });
                    assert!(fees.windows(2).all(|w| w[0] <= w[1]), "{own:?} {sats:?} sending {amount}: {fees:?}");
                }
            }
        }
    }

    #[test]
    fn dust_limits_are_the_nodes() {
        let limit = |a: &str| dust_limit(&Address::parse(a).unwrap());
        assert_eq!(limit(P2WPKH), 294);
        assert_eq!(limit(P2WSH), 330);
        assert_eq!(limit(P2TR), 330);
        assert_eq!(limit(P2SH), 540);
        assert_eq!(limit(P2PKH), 546);
    }

    #[test]
    fn before_a_draft_a_tier_costs_the_typical_send() {
        let set = coins(&[50_000]);
        let q = price(&BtcSendDraft::default(), BtcScriptType::NativeSegwit, Some(&set), Some(AUGUST));
        assert_eq!(fees(&q), [15, 19, 48, 154]);
        assert_eq!(q.dust, None);
        assert_eq!(q.spendable, Some(50_000));
    }

    #[test]
    fn a_taproot_wallet_prices_its_own_typical_send() {
        let q = price(&BtcSendDraft::default(), BtcScriptType::Taproot, None, Some([1.0, 1.0, 1.0, 1.0]));
        assert_eq!(fees(&q), [154; 4]);
        assert_eq!(q.spendable, None);
    }

    #[test]
    fn the_custom_rate_is_priced_like_a_tier() {
        let set = coins(&[500_000]);
        let q = price(&draft(P2WPKH, "0.0025", Some("0.5")), BtcScriptType::NativeSegwit, Some(&set), Some(AUGUST));
        assert_eq!(q.custom.map(|c| c.fee), Some(71));
        assert_eq!(q.custom.map(|c| c.rate), Some(0.5));
        for bad in ["", "abc", "0", "-1", "inf"] {
            let q = price(&draft(P2WPKH, "0.0025", Some(bad)), BtcScriptType::NativeSegwit, Some(&set), Some(AUGUST));
            assert_eq!(q.custom, None, "{bad:?} was priced");
        }
    }

    #[test]
    fn selection_adds_inputs_as_the_amount_grows() {
        let set = coins(&[30_000, 20_000, 5_000]);
        let q = price(&draft(P2WPKH, "0.0001", None), BtcScriptType::NativeSegwit, Some(&set), Some([1.0; 4]));
        assert_eq!(fees(&q)[0], 141);
        let q = price(&draft(P2WPKH, "0.0003", None), BtcScriptType::NativeSegwit, Some(&set), Some([1.0; 4]));
        assert_eq!(fees(&q)[0], vsize_for(&[BtcScriptType::NativeSegwit; 2], &[22, 22]));
    }

    #[test]
    fn max_is_every_coin_less_the_sweep_fee_and_fits_with_one_output() {
        let set = coins(&[30_000, 20_000]);
        let sweep = vsize_for(&[BtcScriptType::NativeSegwit; 2], &[22]);
        let q = price(&draft(P2WPKH, "", None), BtcScriptType::NativeSegwit, Some(&set), Some([1.0; 4]));
        let max = q.tiers.unwrap()[0].max;
        assert_eq!(max, 50_000 - sweep);
        let all = format!("{:.8}", max as f64 / 1e8);
        let q = price(&draft(P2WPKH, &all, None), BtcScriptType::NativeSegwit, Some(&set), Some([1.0; 4]));
        assert_eq!(fees(&q)[0], sweep);
        assert_eq!(max + fees(&q)[0], 50_000);
    }

    #[test]
    fn the_recipient_sets_the_dust_limit_and_the_output_size() {
        let set = coins(&[50_000]);
        let q = price(&draft(P2TR, "0.0001", None), BtcScriptType::NativeSegwit, Some(&set), Some([1.0; 4]));
        assert_eq!(q.dust, Some(330));
        assert_eq!(fees(&q)[0], vsize_for(&[BtcScriptType::NativeSegwit], &[34, 22]));
        let q = price(&draft("bc1qnotanaddress", "0.0001", None), BtcScriptType::NativeSegwit, Some(&set), Some([1.0; 4]));
        assert_eq!(q.dust, None);
        assert_eq!(fees(&q)[0], 141);
    }

    #[test]
    fn no_tiers_and_no_coins_leave_the_figures_unknown() {
        let q = price(&draft(P2WPKH, "0.001", Some("2")), BtcScriptType::NativeSegwit, None, None);
        assert_eq!(q.tiers, None);
        assert_eq!(q.spendable, None);
        assert_eq!(q.custom.map(|c| c.max), Some(0));
        assert_eq!(q.draft, draft(P2WPKH, "0.001", Some("2")));
    }
}
