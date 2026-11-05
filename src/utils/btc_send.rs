use crate::btc_script_type::{self, BtcScriptType};
use crate::channel::CHANNEL;
use crate::ws::commands::bitcoin_payment::{eligible_utxos, tier_sats_at, vsize_for, Utxo};
use dannesk_btc_codec::address::{Address, Kind};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BtcSendDraft {
    pub recipient: String,
    pub amount: String,
    pub custom: Option<String>,
}

/// fee in sats and max.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct FeeQuote {
    pub rate: f32,
    pub fee: u64,
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

/// Bitcoin Core's default dust relay fee in sat/vB.
const DUST_RELAY_FEE: u64 = 3;

pub fn dust_limit(address: &Address) -> u64 {
    let spend = match address.kind() {
        Kind::P2wpkh | Kind::P2wsh | Kind::P2tr | Kind::OtherWitness => 67,
        Kind::P2pkh | Kind::P2sh => 148,
    };
    DUST_RELAY_FEE * (9 + address.script_pubkey().len() as u64 + spend)
}

fn amount_sats(amount: &str) -> u64 {
    amount
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|v| *v > 0.0)
        .map_or(0, |v| (v * 100_000_000.0).round() as u64)
}


fn inputs_for_target(coins: &[Utxo], target: u64) -> Option<usize> {
    let mut total = 0u64;
    for (i, u) in coins.iter().enumerate() {
        total += u.amount;
        if total >= target {
            return Some(i + 1);
        }
    }
    None
}

fn fee_at(rate: f32, coins: &[Utxo], kinds: &[BtcScriptType], own: BtcScriptType, amount: u64, recipient_len: usize) -> u64 {
    let two_out = [recipient_len, own.spk_len()];
    let first = |n: usize| -> Vec<BtcScriptType> {
        if kinds.is_empty() { vec![own; n] } else { kinds[..n.min(kinds.len())].to_vec() }
    };

    let mut n = 1usize;
    let mut fee = tier_sats_at(rate, vsize_for(&first(n), &two_out));
    for _ in 0..8 {
        let needed = inputs_for_target(coins, amount.saturating_add(fee)).unwrap_or_else(|| coins.len().max(1));
        if needed == n {
            break;
        }
        n = needed;
        fee = tier_sats_at(rate, vsize_for(&first(n), &two_out));
    }

    let total: u64 = coins.iter().map(|u| u.amount).sum();
    if amount > 0 && total > 0 && amount.saturating_add(fee) > total {
        let fee1 = tier_sats_at(rate, vsize_for(kinds, &[recipient_len]));
        if amount.saturating_add(fee1) <= total {
            return fee1;
        }
    }
    fee
}

pub fn price(draft: &BtcSendDraft, own: BtcScriptType, coins: Option<&[Utxo]>, tiers: Option<[f32; 4]>) -> BtcSendQuote {
    let held = coins.unwrap_or(&[]);
    let kinds: Vec<BtcScriptType> = held.iter().map(|c| BtcScriptType::of_address(&c.address).unwrap_or(own)).collect();
    let spendable = coins.map(|c| c.iter().map(|u| u.amount).sum::<u64>());
    let recipient = Address::parse(draft.recipient.trim()).ok();
    let recipient_len = recipient.as_ref().map_or(own.spk_len(), |a| a.script_pubkey().len());
    let amount = amount_sats(&draft.amount);
    let sweep = vsize_for(&kinds, &[recipient_len]);
    let quote = |rate: f32| FeeQuote {
        rate,
        fee: fee_at(rate, held, &kinds, own, amount, recipient_len),
        max: spendable.unwrap_or(0).saturating_sub(tier_sats_at(rate, sweep)),
    };
    let custom = draft
        .custom
        .as_deref()
        .and_then(|r| r.trim().parse::<f32>().ok())
        .filter(|r| r.is_finite() && *r > 0.0);
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
        sats.iter()
            .enumerate()
            .map(|(i, s)| Utxo { txid: format!("{:02x}", 0x11 + i).repeat(32), vout: 0, amount: *s, address: OWN.to_string() })
            .collect()
    }

    fn draft(recipient: &str, amount: &str, custom: Option<&str>) -> BtcSendDraft {
        BtcSendDraft { recipient: recipient.to_string(), amount: amount.to_string(), custom: custom.map(String::from) }
    }

    fn fees(q: &BtcSendQuote) -> [u64; 4] {
        q.tiers.unwrap().map(|t| t.fee)
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
