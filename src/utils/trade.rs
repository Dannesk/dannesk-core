// trade.rs

use serde::{Deserialize, Serialize};

use crate::channel::{CHANNEL, OrderBook};
use crate::utils::liquidity;
use crate::utils::orderbook::{ceil_to, floor_sig, floor_to, mid_and_spread, oriented, touch, walk, Quote};

pub const SIGN_PLACES: usize = 6;
pub const BOUND_GUARD_PCT: f64 = 5.0;
pub const FRAME_STALE_LEDGERS: u64 = 2;
const CUSHION_ALPHA: f64 = 0.5;
const CUSHION_MAX: f64 = 0.01;
const QUOTE_LEVELS: usize = 24;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TradeSide {
    Buy,
    Sell,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TradeKind {
    Market,
    Limit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TradeTif {
    Gtc,
    Ioc,
    Fok,
}

// amount is in the base asset and limit is quote per base.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TradeTicket {
    pub base: String,
    pub quote: String,
    pub side: TradeSide,
    pub kind: TradeKind,
    pub amount: String,
    pub limit: Option<f64>,
    pub tif: TradeTif,
}

impl TradeTicket {
    pub fn pay_recv(&self) -> (&str, &str) {
        match self.side {
            TradeSide::Sell => (self.base.as_str(), self.quote.as_str()),
            TradeSide::Buy => (self.quote.as_str(), self.base.as_str()),
        }
    }

    // Selling types the pay side and buying types the receive side
    fn anchor_is_pay(&self) -> bool {
        self.side == TradeSide::Sell
    }

    fn typed(&self) -> f64 {
        self.amount.trim().parse().unwrap_or(0.0)
    }

    // Internal rates are receive per pay and displayed prices are quote per base
    fn market_price(&self, internal: f64) -> f64 {
        match self.side {
            TradeSide::Sell => internal,
            TradeSide::Buy => {
                if internal > 0.0 {
                    1.0 / internal
                } else {
                    0.0
                }
            }
        }
    }

    fn internal_limit(&self) -> f64 {
        self.limit.map(|l| self.market_price(l)).unwrap_or(0.0)
    }
}

// Desktop's market mode's state against the live book
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Verdict {
    LimitOnly,
    Measuring,
    NoFrame,
    Idle,
    NoDepth,
    Walked { coverage_pct: f64, bound_pct: f64, held: bool },
}


#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TradeQuote {
    pub ticket: TradeTicket,
    pub market_ok: bool,
    pub require_auth: bool,
    pub global_freeze: bool,
    pub bids: Vec<(f64, f64)>,
    pub asks: Vec<(f64, f64)>,
    pub book_mid: Option<f64>,
    pub book_spread_pct: Option<f64>,
    pub ledger: u64,
    pub fresh: bool,
    pub verdict: Verdict,
    pub price_row: Option<f64>,
    pub counter: Option<f64>,
    pub route: Option<String>,
    pub pay_asset: String,
    pub receive_asset: String,
    pub pay: Option<String>,
    pub receive: Option<String>,
    pub flags: Vec<String>,
    pub lockable: bool,
    pub max_base: Option<f64>,
}

fn frame_ok(frame: u64) -> bool {
    let live = CHANNEL.xrp_node_rx.borrow().ledger_index.unwrap_or(0);
    if frame == 0 || live == 0 {
        return false;
    }
    frame + FRAME_STALE_LEDGERS >= live
}

fn issuer_restricted(book: &OrderBook) -> bool {
    book.require_auth == Some(true) || book.global_freeze == Some(true)
}

// Fixed per pair so the control never flickers with the book's depth on each ledger.
fn market_offered(t: &TradeTicket) -> bool {
    let (pay, recv) = t.pay_recv();
    if pay.is_empty() || recv.is_empty() || (pay != "XRP" && recv != "XRP") {
        return false;
    }
    !oriented(pay, recv).is_some_and(|(b, _)| issuer_restricted(&b))
}

fn quotable(t: &TradeTicket) -> bool {
    if !market_offered(t) {
        return false;
    }
    let (pay, recv) = t.pay_recv();
    oriented(pay, recv).is_some_and(|(b, _)| touch(&b).is_some() && frame_ok(b.ledger))
}

// The walk for the typed amount
fn book_quote(t: &TradeTicket) -> Option<Quote> {
    let (pay, recv) = t.pay_recv();
    let typed = t.typed();
    if typed <= 0.0 {
        let (book, _) = oriented(pay, recv)?;
        let touch = touch(&book)?;
        return Some(Quote {
            vwap: touch,
            floor: touch,
            filled: 0.0,
            receive: 0.0,
            depth_short: false,
            ledger: book.ledger,
            from_pool: 0.0,
            clob_levels: 0,
        });
    }
    let q = walk(pay, recv, typed, t.anchor_is_pay())?;
    frame_ok(q.ledger).then_some(q)
}

fn touch_rate(t: &TradeTicket) -> f64 {
    let (pay, recv) = t.pay_recv();
    oriented(pay, recv).and_then(|(b, _)| touch(&b)).unwrap_or(0.0)
}

fn tick_size(t: &TradeTicket) -> Option<u32> {
    let (pay, recv) = t.pay_recv();
    oriented(pay, recv).and_then(|(b, _)| b.tick_size)
}

fn delta_min(tick_size: Option<u32>) -> f64 {
    let by_tick = tick_size
        .filter(|n| *n >= 1)
        .map(|n| 10f64.powi(-((n as i32) - 1)))
        .unwrap_or(0.0);
    by_tick.max(1e-7)
}

fn cushioned(floor: f64, touch: f64, tick_size: Option<u32>) -> f64 {
    if floor <= 0.0 {
        return 0.0;
    }
    let slope = if touch > 0.0 { ((touch - floor) / touch).max(0.0) } else { 0.0 };
    let d = (CUSHION_ALPHA * slope).clamp(delta_min(tick_size), CUSHION_MAX);
    floor_sig(floor * (1.0 - d), 9)
}

fn bound_vs_mid(t: &TradeTicket, bound: f64) -> Option<f64> {
    let m = liquidity::market(&t.base, &t.quote)?;
    if m.mid <= 0.0 || bound <= 0.0 {
        return None;
    }
    Some((t.market_price(bound) - m.mid) / m.mid * 100.0)
}

fn bound_sane(t: &TradeTicket, bound: f64) -> bool {
    let Some(m) = liquidity::market(&t.base, &t.quote) else { return false };
    match bound_vs_mid(t, bound) {
        Some(d) => d.abs() <= BOUND_GUARD_PCT || m.stable,
        None => false,
    }
}

fn market_bound(t: &TradeTicket, q: &Quote) -> f64 {
    cushioned(q.floor, touch_rate(t), tick_size(t))
}

pub fn verdict(t: &TradeTicket) -> Verdict {
    if !market_offered(t) {
        return Verdict::LimitOnly;
    }
    if liquidity::market(&t.base, &t.quote).is_none() {
        return Verdict::Measuring;
    }
    if t.typed() <= 0.0 {
        return Verdict::Idle;
    }
    let (pay, recv) = t.pay_recv();
    let Some((book, _)) = oriented(pay, recv) else { return Verdict::NoFrame };
    if !frame_ok(book.ledger) {
        return Verdict::NoFrame;
    }
    let Some(q) = book_quote(t) else { return Verdict::NoDepth };
    let got = if t.anchor_is_pay() { q.filled } else { q.receive };
    let bound = market_bound(t, &q);
    Verdict::Walked {
        coverage_pct: coverage_pct(got, t.typed()),
        bound_pct: bound_vs_mid(t, bound).unwrap_or(0.0),
        held: !bound_sane(t, bound),
    }
}

fn coverage_pct(got: f64, want: f64) -> f64 {
    if want <= 0.0 {
        return 0.0;
    }
    (got / want * 100.0).clamp(0.0, 100.0)
}

// The internal rate a signature binds...zero while there is none
fn effective_limit(t: &TradeTicket) -> f64 {
    match t.kind {
        TradeKind::Market => match book_quote(t).filter(|_| quotable(t)) {
            Some(q) => {
                let b = market_bound(t, &q);
                if bound_sane(t, b) { b } else { 0.0 }
            }
            None => 0.0,
        },
        TradeKind::Limit => t.internal_limit(),
    }
}

#[derive(Debug, Clone, Copy)]
struct Terms {
    pay: f64,
    receive: f64,
}

fn pays_whole_balance(t: &TradeTicket) -> bool {
    let (pay, _) = t.pay_recv();
    if pay == "XRP" || !t.anchor_is_pay() {
        return false;
    }
    let balance = CHANNEL.token(pay).0;
    balance > 0.0 && t.amount.trim().parse::<f64>().ok() == Some(balance)
}

// rounds down when typed and up when derived, so bound always favours the signer
fn terms(t: &TradeTicket, limit: f64) -> Terms {
    if t.anchor_is_pay() {
        let typed = t.typed();
        let pay = if pays_whole_balance(t) { typed } else { floor_to(typed, SIGN_PLACES) };
        let receive = if limit > 0.0 { floor_to(pay * limit, SIGN_PLACES) } else { 0.0 };
        Terms { pay, receive }
    } else {
        let receive = floor_to(t.typed(), SIGN_PLACES);
        let pay = if limit > 0.0 { ceil_to(receive / limit, SIGN_PLACES) } else { 0.0 };
        Terms { pay, receive }
    }
}

fn amount_string(x: f64) -> String {
    let raw = format!("{:.*}", SIGN_PLACES, x);
    raw.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn exact_amount(v: f64) -> String {
    let s = format!("{v}");
    s.strip_suffix(".0").map(str::to_string).unwrap_or(s)
}

fn xrp_fee() -> f64 {
    CHANNEL.xrp_node_rx.borrow().open_ledger_fee.unwrap_or(0) as f64 / 1e6
}

fn per_object_reserve() -> Option<f64> {
    CHANNEL.xrp_node_rx.borrow().reserve_inc.map(|d| d as f64 / 1e6)
}

fn xrp_available() -> Option<f64> {
    let balance = CHANNEL.wallet_balance_rx.borrow().0;
    let node = CHANNEL.xrp_node_rx.borrow();
    let (base, inc) = (node.reserve_base?, node.reserve_inc?);
    let owners = CHANNEL.xrp_account_rx.borrow().owner_count.unwrap_or(0);
    Some(balance - (base as f64 + owners as f64 * inc as f64) / 1e6)
}

fn can_rest(t: &TradeTicket) -> bool {
    t.kind == TradeKind::Limit && t.tif == TradeTif::Gtc
}

pub fn flags(t: &TradeTicket) -> Vec<String> {
    let mut flags = match t.kind {
        TradeKind::Market => vec!["tfImmediateOrCancel".to_string()],
        TradeKind::Limit => match t.tif {
            TradeTif::Gtc => Vec::new(),
            TradeTif::Ioc => vec!["tfImmediateOrCancel".to_string()],
            TradeTif::Fok => vec!["tfFillOrKill".to_string()],
        },
    };
    if t.anchor_is_pay() {
        flags.push("tfSell".to_string());
    }
    flags
}

// spend, after the fee and the reserve...
fn available_pay(t: &TradeTicket) -> Option<f64> {
    let (pay, _) = t.pay_recv();
    if pay == "XRP" {
        let owed = xrp_fee() + if can_rest(t) { per_object_reserve()? } else { 0.0 };
        Some((xrp_available()? - owed).max(0.0))
    } else {
        Some(CHANNEL.token(pay).0)
    }
}

// The largest base amount the ticket could ask for, rounded so the paid side never exceeds the balance
fn max_base(t: &TradeTicket) -> Option<f64> {
    let avail = available_pay(t)?;
    match t.side {
        TradeSide::Sell => Some(avail),
        TradeSide::Buy => {
            let rate = match t.kind {
                TradeKind::Limit => {
                    let l = t.internal_limit();
                    if l <= 0.0 {
                        return None;
                    }
                    l
                }
                TradeKind::Market => {
                    if avail <= 0.0 || !quotable(t) {
                        return None;
                    }
                    let (pay, recv) = t.pay_recv();
                    let q = walk(pay, recv, avail, true)?;
                    let b = market_bound(t, &q);
                    if b <= 0.0 {
                        return None;
                    }
                    b
                }
            };
            let mut max = floor_to(avail * rate, SIGN_PLACES);
            for _ in 0..4 {
                if max <= 0.0 {
                    break;
                }
                if ceil_to(max / rate, SIGN_PLACES) <= avail {
                    break;
                }
                max = floor_to(max - 10f64.powi(-(SIGN_PLACES as i32)), SIGN_PLACES);
            }
            Some(max.max(0.0))
        }
    }
}

fn route(q: &Quote) -> String {
    match (q.clob_levels > 0, q.from_pool > 0.0) {
        (true, true) => "Book + pool".to_string(),
        (false, true) => "Pool".to_string(),
        _ => "Book".to_string(),
    }
}

pub fn compute(t: &TradeTicket) -> TradeQuote {
    let (pay_asset, receive_asset) = {
        let (p, r) = t.pay_recv();
        (p.to_string(), r.to_string())
    };
    let book = oriented(&t.base, &t.quote);
    let (bids, asks, book_mid, book_spread_pct, ledger, require_auth, global_freeze) = match &book {
        Some((b, _)) => {
            let ms = mid_and_spread(b);
            (
                b.bids.iter().take(QUOTE_LEVELS).copied().collect(),
                b.asks.iter().take(QUOTE_LEVELS).copied().collect(),
                ms.map(|(m, _)| m),
                ms.map(|(_, s)| s),
                b.ledger,
                b.require_auth == Some(true),
                b.global_freeze == Some(true),
            )
        }
        None => (Vec::new(), Vec::new(), None, None, 0, false, false),
    };

    let verdict = verdict(t);
    let quote = book_quote(t);
    let limit = effective_limit(t);
    let tm = terms(t, limit);
    let signable = limit > 0.0 && tm.pay > 0.0 && tm.receive > 0.0;

    let price_row = match t.kind {
        TradeKind::Market => quote.as_ref().filter(|q| q.vwap > 0.0).map(|q| t.market_price(q.vwap)),
        TradeKind::Limit => None,
    };
    let counter = match t.kind {
        TradeKind::Market => match (&verdict, &quote) {
            (Verdict::Walked { .. }, Some(q)) => Some(match t.side {
                TradeSide::Sell => q.receive,
                TradeSide::Buy => q.filled,
            }),
            _ => None,
        },
        TradeKind::Limit => signable.then(|| match t.side {
            TradeSide::Sell => tm.receive,
            TradeSide::Buy => tm.pay,
        }),
    };
    let route = match (t.kind, &verdict, &quote) {
        (TradeKind::Market, Verdict::Walked { .. }, Some(q)) => Some(route(q)),
        _ => None,
    };
    let (pay, receive) = if signable {
        let pay = if pays_whole_balance(t) { exact_amount(tm.pay) } else { amount_string(tm.pay) };
        (Some(pay), Some(amount_string(tm.receive)))
    } else {
        (None, None)
    };
    let lockable = signable
        && !global_freeze
        && match t.kind {
            TradeKind::Market => matches!(verdict, Verdict::Walked { held: false, .. }),
            TradeKind::Limit => t.limit.is_some(),
        };

    TradeQuote {
        market_ok: market_offered(t),
        require_auth,
        global_freeze,
        bids,
        asks,
        book_mid,
        book_spread_pct,
        ledger,
        fresh: frame_ok(ledger),
        verdict,
        price_row,
        counter,
        route,
        pay_asset,
        receive_asset,
        pay,
        receive,
        flags: flags(t),
        lockable,
        max_base: max_base(t),
        ticket: t.clone(),
    }
}

// The legs: one for XRP pair and both for a cross
fn wanted_books(t: &TradeTicket) -> Vec<String> {
    let mut want = Vec::new();
    for leg in [&t.base, &t.quote] {
        if leg != "XRP" {
            want.push(format!("XRP/{leg}"));
        }
    }
    want
}

fn sync_books(subscribed: &mut Vec<String>, ticket: Option<&TradeTicket>) {
    use crate::ws::{rates_send, RatesCommand};
    let wanted = ticket.map(wanted_books).unwrap_or_default();
    for old in subscribed.iter() {
        if !wanted.contains(old) {
            rates_send(RatesCommand::UnsubscribeBook(old.clone()));
        }
    }
    for new in &wanted {
        if !subscribed.contains(new) {
            rates_send(RatesCommand::SubscribeBook(new.clone()));
        }
    }
    *subscribed = wanted;
}


pub async fn quote_task() {
    let mut subscribed: Vec<String> = Vec::new();
    let mut ticket_rx = CHANNEL.trade_ticket_rx.clone();
    let mut book_rx = CHANNEL.orderbook_rx.clone();
    let mut markets_rx = CHANNEL.markets_rx.clone();
    let mut node_rx = CHANNEL.xrp_node_rx.clone();
    let mut tokens_rx = CHANNEL.tokens_rx.clone();
    let mut wallet_rx = CHANNEL.wallet_balance_rx.clone();
    let mut account_rx = CHANNEL.xrp_account_rx.clone();
    loop {
        tokio::select! {
            r = ticket_rx.changed() => { if r.is_err() { return; } }
            r = book_rx.changed() => { if r.is_err() { return; } }
            r = markets_rx.changed() => { if r.is_err() { return; } }
            r = node_rx.changed() => { if r.is_err() { return; } }
            r = tokens_rx.changed() => { if r.is_err() { return; } }
            r = wallet_rx.changed() => { if r.is_err() { return; } }
            r = account_rx.changed() => { if r.is_err() { return; } }
        }
        let ticket = ticket_rx.borrow_and_update().clone();
        sync_books(&mut subscribed, ticket.as_ref());
        book_rx.mark_unchanged();
        markets_rx.mark_unchanged();
        node_rx.mark_unchanged();
        tokens_rx.mark_unchanged();
        wallet_rx.mark_unchanged();
        account_rx.mark_unchanged();
        let quote = ticket.as_ref().map(compute);
        CHANNEL.trade_quote_tx.send_if_modified(|q| {
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

    fn ticket(side: TradeSide, kind: TradeKind, amount: &str, limit: Option<f64>, tif: TradeTif) -> TradeTicket {
        TradeTicket {
            base: "XRP".to_string(),
            quote: "RLUSD".to_string(),
            side,
            kind,
            amount: amount.to_string(),
            limit,
            tif,
        }
    }

    #[test]
    fn a_sell_types_the_pay_side_and_a_buy_the_receive_side() {
        let sell = ticket(TradeSide::Sell, TradeKind::Limit, "250", Some(1.3004), TradeTif::Gtc);
        assert_eq!(sell.pay_recv(), ("XRP", "RLUSD"));
        assert!(sell.anchor_is_pay());
        let buy = ticket(TradeSide::Buy, TradeKind::Limit, "250", Some(1.3004), TradeTif::Gtc);
        assert_eq!(buy.pay_recv(), ("RLUSD", "XRP"));
        assert!(!buy.anchor_is_pay());
    }

    #[test]
    fn the_internal_rate_is_receive_per_pay_on_both_sides() {
        let sell = ticket(TradeSide::Sell, TradeKind::Limit, "1000", Some(1.3004), TradeTif::Gtc);
        assert!((sell.internal_limit() - 1.3004).abs() < 1e-12);
        let buy = ticket(TradeSide::Buy, TradeKind::Limit, "1000", Some(1.3004), TradeTif::Gtc);
        assert!((buy.internal_limit() - 1.0 / 1.3004).abs() < 1e-12);
        assert!((buy.market_price(buy.internal_limit()) - 1.3004).abs() < 1e-9);
    }

    #[test]
    fn terms_round_against_the_signer_on_the_derived_side() {
        let sell = ticket(TradeSide::Sell, TradeKind::Limit, "1000", Some(1.3004), TradeTif::Gtc);
        let tm = terms(&sell, sell.internal_limit());
        assert_eq!(tm.pay, 1000.0);
        assert_eq!(tm.receive, 1300.4);
        let buy = ticket(TradeSide::Buy, TradeKind::Limit, "1000", Some(1.3004), TradeTif::Gtc);
        let tm = terms(&buy, buy.internal_limit());
        assert_eq!(tm.receive, 1000.0);
        assert!(tm.pay >= 1300.4 && tm.pay < 1300.400001, "{}", tm.pay);
    }

    #[test]
    fn the_flags_carry_the_tif_and_the_sell_side_carries_tfsell() {
        let gtc = ticket(TradeSide::Sell, TradeKind::Limit, "1", Some(1.3), TradeTif::Gtc);
        assert_eq!(flags(&gtc), vec!["tfSell".to_string()]);
        let ioc = ticket(TradeSide::Sell, TradeKind::Limit, "1", Some(1.3), TradeTif::Ioc);
        assert_eq!(flags(&ioc), vec!["tfImmediateOrCancel".to_string(), "tfSell".to_string()]);
        let fok_buy = ticket(TradeSide::Buy, TradeKind::Limit, "1", Some(1.3), TradeTif::Fok);
        assert_eq!(flags(&fok_buy), vec!["tfFillOrKill".to_string()]);
        let market_buy = ticket(TradeSide::Buy, TradeKind::Market, "1", None, TradeTif::Gtc);
        assert_eq!(flags(&market_buy), vec!["tfImmediateOrCancel".to_string()]);
    }

    #[test]
    fn only_a_resting_limit_holds_the_reserve() {
        assert!(can_rest(&ticket(TradeSide::Sell, TradeKind::Limit, "1", Some(1.3), TradeTif::Gtc)));
        assert!(!can_rest(&ticket(TradeSide::Sell, TradeKind::Limit, "1", Some(1.3), TradeTif::Ioc)));
        assert!(!can_rest(&ticket(TradeSide::Sell, TradeKind::Limit, "1", Some(1.3), TradeTif::Fok)));
        assert!(!can_rest(&ticket(TradeSide::Sell, TradeKind::Market, "1", None, TradeTif::Gtc)));
    }

    #[test]
    fn the_cushion_stays_between_its_floor_and_its_cap() {
        let tight = cushioned(1.3000, 1.3001, None);
        assert!(tight < 1.3000 && tight > 1.3000 * (1.0 - CUSHION_MAX));
        let wide = cushioned(1.0, 2.0, None);
        assert!((wide - 1.0 * (1.0 - CUSHION_MAX)).abs() < 1e-9);
        assert_eq!(cushioned(0.0, 1.0, None), 0.0);
    }

    #[test]
    fn a_ticket_parses_from_the_apps_json() {
        let json = r#"{"base":"XRP","quote":"RLUSD","side":"sell","kind":"limit","amount":"250","limit":1.3004,"tif":"gtc"}"#;
        let t: TradeTicket = serde_json::from_str(json).unwrap();
        assert_eq!(t.side, TradeSide::Sell);
        assert_eq!(t.kind, TradeKind::Limit);
        assert_eq!(t.tif, TradeTif::Gtc);
        assert_eq!(t.limit, Some(1.3004));
        let market = r#"{"base":"XRP","quote":"RLUSD","side":"buy","kind":"market","amount":"","limit":null,"tif":"gtc"}"#;
        let t: TradeTicket = serde_json::from_str(market).unwrap();
        assert_eq!(t.kind, TradeKind::Market);
        assert_eq!(t.limit, None);
    }

    #[test]
    fn the_wire_strings_trim_as_desktops() {
        assert_eq!(amount_string(1300.4), "1300.4");
        assert_eq!(amount_string(250.0), "250");
        assert_eq!(amount_string(floor_to(0.1234567, SIGN_PLACES)), "0.123456");
        assert_eq!(exact_amount(8.342663), "8.342663");
        assert_eq!(exact_amount(5.0), "5");
    }
}
