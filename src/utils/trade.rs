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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TradeUnit {
    Base,
    Quote,
}

// amount is in the unit typed, limit is quote per base, and side and kind stay None until user picks one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TradeTicket {
    pub base: String,
    pub quote: String,
    pub side: Option<TradeSide>,
    pub kind: Option<TradeKind>,
    pub unit: TradeUnit,
    pub amount: String,
    pub limit: Option<f64>,
    pub tif: TradeTif,
}

impl TradeTicket {
    pub fn pay_recv(&self) -> Option<(&str, &str)> {
        Some(match self.side? {
            TradeSide::Sell => (self.base.as_str(), self.quote.as_str()),
            TradeSide::Buy => (self.quote.as_str(), self.base.as_str()),
        })
    }

    // The typed amount is the one paid when it is the base on a sell or the quote on a buy
    pub fn anchor_is_pay(&self) -> bool {
        matches!(
            (self.side, self.unit),
            (Some(TradeSide::Sell), TradeUnit::Base) | (Some(TradeSide::Buy), TradeUnit::Quote)
        )
    }
}


#[derive(Clone, Copy)]
struct Order<'a> {
    t: &'a TradeTicket,
    side: TradeSide,
    kind: TradeKind,
}

impl<'a> Order<'a> {
    fn of(t: &'a TradeTicket) -> Option<Self> {
        Some(Order { t, side: t.side?, kind: t.kind? })
    }

    fn pay_recv(&self) -> (&'a str, &'a str) {
        match self.side {
            TradeSide::Sell => (self.t.base.as_str(), self.t.quote.as_str()),
            TradeSide::Buy => (self.t.quote.as_str(), self.t.base.as_str()),
        }
    }

    fn anchor_is_pay(&self) -> bool {
        self.t.anchor_is_pay()
    }

    fn typed(&self) -> f64 {
        self.t.amount.trim().parse().unwrap_or(0.0)
    }

    // rates are receive per pay and displayed prices are quote per base
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
        self.t.limit.map(|l| self.market_price(l)).unwrap_or(0.0)
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
    Walked { coverage_pct: f64, bound_pct: f64, off_market: bool, unstable: bool },
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
    pub max: Option<f64>,
    pub max_pay: Option<f64>,
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
fn market_offered(base: &str, quote: &str) -> bool {
    if base.is_empty() || quote.is_empty() || (base != "XRP" && quote != "XRP") {
        return false;
    }
    !oriented(base, quote).is_some_and(|(b, _)| issuer_restricted(&b))
}

fn quotable(o: Order) -> bool {
    if !market_offered(&o.t.base, &o.t.quote) {
        return false;
    }
    let (pay, recv) = o.pay_recv();
    oriented(pay, recv).is_some_and(|(b, _)| touch(&b).is_some() && frame_ok(b.ledger))
}

// The walk for the typed amount
fn book_quote(o: Order) -> Option<Quote> {
    let (pay, recv) = o.pay_recv();
    let typed = o.typed();
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
    let q = walk(pay, recv, typed, o.anchor_is_pay())?;
    frame_ok(q.ledger).then_some(q)
}

fn touch_rate(o: Order) -> f64 {
    let (pay, recv) = o.pay_recv();
    oriented(pay, recv).and_then(|(b, _)| touch(&b)).unwrap_or(0.0)
}

fn tick_size(o: Order) -> Option<u32> {
    let (pay, recv) = o.pay_recv();
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

fn bound_vs_mid(o: Order, bound: f64) -> Option<f64> {
    let m = liquidity::market(&o.t.base, &o.t.quote)?;
    if m.mid <= 0.0 || bound <= 0.0 {
        return None;
    }
    Some((o.market_price(bound) - m.mid) / m.mid * 100.0)
}

fn market_bound(o: Order, q: &Quote) -> f64 {
    cushioned(q.floor, touch_rate(o), tick_size(o))
}

// An off-market walk and an unstable book are warnings, never a reason to refuse the order
fn verdict(o: Order) -> Verdict {
    if !market_offered(&o.t.base, &o.t.quote) {
        return Verdict::LimitOnly;
    }
    let Some(m) = liquidity::market(&o.t.base, &o.t.quote) else { return Verdict::Measuring };
    if o.typed() <= 0.0 {
        return Verdict::Idle;
    }
    let (pay, recv) = o.pay_recv();
    let Some((book, _)) = oriented(pay, recv) else { return Verdict::NoFrame };
    if !frame_ok(book.ledger) {
        return Verdict::NoFrame;
    }
    let Some(q) = book_quote(o) else { return Verdict::NoDepth };
    let got = if o.anchor_is_pay() { q.filled } else { q.receive };
    let vs_mid = bound_vs_mid(o, market_bound(o, &q));
    Verdict::Walked {
        coverage_pct: coverage_pct(got, o.typed()),
        bound_pct: vs_mid.unwrap_or(0.0),
        off_market: vs_mid.is_some_and(|d| d.abs() > BOUND_GUARD_PCT),
        unstable: !m.stable,
    }
}

fn coverage_pct(got: f64, want: f64) -> f64 {
    if want <= 0.0 {
        return 0.0;
    }
    (got / want * 100.0).clamp(0.0, 100.0)
}

// The internal rate a signature binds...zero while there is none
fn effective_limit(o: Order) -> f64 {
    match o.kind {
        TradeKind::Market => book_quote(o).filter(|_| quotable(o)).map_or(0.0, |q| market_bound(o, &q)),
        TradeKind::Limit => o.internal_limit(),
    }
}

#[derive(Debug, Clone, Copy)]
struct Terms {
    pay: f64,
    receive: f64,
}

fn pays_whole_balance(o: Order) -> bool {
    let (pay, _) = o.pay_recv();
    if pay == "XRP" || !o.anchor_is_pay() {
        return false;
    }
    let (balance, _, _, exact) = CHANNEL.token(pay);
    balance > 0.0 && o.t.amount.trim() == exact
}

// rounds down when typed and up when derived, so bound always favours the signer
fn terms(o: Order, limit: f64) -> Terms {
    if o.anchor_is_pay() {
        let typed = o.typed();
        let pay = if pays_whole_balance(o) { typed } else { floor_to(typed, SIGN_PLACES) };
        let receive = if limit > 0.0 { floor_to(pay * limit, SIGN_PLACES) } else { 0.0 };
        Terms { pay, receive }
    } else {
        let receive = floor_to(o.typed(), SIGN_PLACES);
        let pay = if limit > 0.0 { ceil_to(receive / limit, SIGN_PLACES) } else { 0.0 };
        Terms { pay, receive }
    }
}

fn amount_string(x: f64) -> String {
    let raw = format!("{:.*}", SIGN_PLACES, x);
    raw.trim_end_matches('0').trim_end_matches('.').to_string()
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

fn can_rest(o: Order) -> bool {
    o.kind == TradeKind::Limit && o.t.tif == TradeTif::Gtc
}

fn flags(o: Order) -> Vec<String> {
    let mut flags = match o.kind {
        TradeKind::Market => vec!["tfImmediateOrCancel".to_string()],
        TradeKind::Limit => match o.t.tif {
            TradeTif::Gtc => Vec::new(),
            TradeTif::Ioc => vec!["tfImmediateOrCancel".to_string()],
            TradeTif::Fok => vec!["tfFillOrKill".to_string()],
        },
    };
    if o.anchor_is_pay() {
        flags.push("tfSell".to_string());
    }
    flags
}

// spend, after the fee and the reserve...
fn available_pay(o: Order) -> Option<f64> {
    let (pay, _) = o.pay_recv();
    if pay == "XRP" {
        let owed = xrp_fee() + if can_rest(o) { per_object_reserve()? } else { 0.0 };
        Some((xrp_available()? - owed).max(0.0))
    } else {
        Some(CHANNEL.token(pay).0)
    }
}

// The largest typed amount the ticket could ask for, rounded so the paid side never exceeds the balance
fn max_typed(o: Order) -> Option<f64> {
    let avail = available_pay(o)?;
    if o.anchor_is_pay() {
        return Some(avail);
    }
    let rate = match o.kind {
        TradeKind::Limit => {
            let l = o.internal_limit();
            if l <= 0.0 {
                return None;
            }
            l
        }
        TradeKind::Market => {
            if avail <= 0.0 || !quotable(o) {
                return None;
            }
            let (pay, recv) = o.pay_recv();
            let q = walk(pay, recv, avail, true)?;
            let b = market_bound(o, &q);
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

fn route(q: &Quote) -> String {
    match (q.clob_levels > 0, q.from_pool > 0.0) {
        (true, true) => "Book + pool".to_string(),
        (false, true) => "Pool".to_string(),
        _ => "Book".to_string(),
    }
}

pub fn compute(t: &TradeTicket) -> TradeQuote {
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
    let market_ok = market_offered(&t.base, &t.quote);
    let (pay_asset, receive_asset) = t
        .pay_recv()
        .map_or((String::new(), String::new()), |(p, r)| (p.to_string(), r.to_string()));
    let mut quote = TradeQuote {
        ticket: t.clone(),
        market_ok,
        require_auth,
        global_freeze,
        bids,
        asks,
        book_mid,
        book_spread_pct,
        ledger,
        fresh: frame_ok(ledger),
        verdict: if market_ok { Verdict::Idle } else { Verdict::LimitOnly },
        price_row: None,
        counter: None,
        route: None,
        pay_asset,
        receive_asset,
        pay: None,
        receive: None,
        flags: Vec::new(),
        lockable: false,
        max: None,
        max_pay: None,
    };
    let Some(o) = Order::of(t) else { return quote };

    let verdict = verdict(o);
    let walked = book_quote(o);
    let limit = effective_limit(o);
    let tm = terms(o, limit);
    let signable = limit > 0.0 && tm.pay > 0.0 && tm.receive > 0.0;

    quote.price_row = match o.kind {
        TradeKind::Market => walked.as_ref().filter(|q| q.vwap > 0.0).map(|q| o.market_price(q.vwap)),
        TradeKind::Limit => None,
    };
    quote.counter = match o.kind {
        TradeKind::Market => match (&verdict, &walked) {
            (Verdict::Walked { .. }, Some(q)) => Some(if o.anchor_is_pay() { q.receive } else { q.filled }),
            _ => None,
        },
        TradeKind::Limit => signable.then(|| if o.anchor_is_pay() { tm.receive } else { tm.pay }),
    };
    quote.route = match (o.kind, &verdict, &walked) {
        (TradeKind::Market, Verdict::Walked { .. }, Some(q)) => Some(route(q)),
        _ => None,
    };
    if signable {
        quote.pay = Some(if pays_whole_balance(o) { CHANNEL.token(&quote.pay_asset).3 } else { amount_string(tm.pay) });
        quote.receive = Some(amount_string(tm.receive));
    }
    quote.lockable = signable
        && !quote.global_freeze
        && match o.kind {
            TradeKind::Market => matches!(verdict, Verdict::Walked { .. }),
            TradeKind::Limit => t.limit.is_some(),
        };
    quote.verdict = verdict;
    quote.flags = flags(o);
    quote.max = max_typed(o);
    quote.max_pay = available_pay(o);
    quote
}

// legs: one for XRP pair and both for a cross
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
    use crate::channel::MarketSummary;

    fn ticket(side: TradeSide, kind: TradeKind, unit: TradeUnit, amount: &str, limit: Option<f64>, tif: TradeTif) -> TradeTicket {
        TradeTicket {
            base: "XRP".to_string(),
            quote: "RLUSD".to_string(),
            side: Some(side),
            kind: Some(kind),
            unit,
            amount: amount.to_string(),
            limit,
            tif,
        }
    }

    fn order(t: &TradeTicket) -> Order<'_> {
        Order::of(t).expect("side and kind picked")
    }

    #[test]
    fn the_typed_side_is_paid_when_it_is_the_base_on_a_sell_or_the_quote_on_a_buy() {
        let sell = ticket(TradeSide::Sell, TradeKind::Limit, TradeUnit::Base, "250", Some(1.3004), TradeTif::Gtc);
        assert_eq!(sell.pay_recv(), Some(("XRP", "RLUSD")));
        assert!(sell.anchor_is_pay());
        let buy = ticket(TradeSide::Buy, TradeKind::Limit, TradeUnit::Base, "250", Some(1.3004), TradeTif::Gtc);
        assert_eq!(buy.pay_recv(), Some(("RLUSD", "XRP")));
        assert!(!buy.anchor_is_pay());
        assert!(!ticket(TradeSide::Sell, TradeKind::Limit, TradeUnit::Quote, "1", None, TradeTif::Gtc).anchor_is_pay());
        assert!(ticket(TradeSide::Buy, TradeKind::Limit, TradeUnit::Quote, "1", None, TradeTif::Gtc).anchor_is_pay());
    }

    #[test]
    fn the_internal_rate_is_receive_per_pay_on_both_sides() {
        let sell = ticket(TradeSide::Sell, TradeKind::Limit, TradeUnit::Base, "1000", Some(1.3004), TradeTif::Gtc);
        assert!((order(&sell).internal_limit() - 1.3004).abs() < 1e-12);
        let buy = ticket(TradeSide::Buy, TradeKind::Limit, TradeUnit::Base, "1000", Some(1.3004), TradeTif::Gtc);
        let o = order(&buy);
        assert!((o.internal_limit() - 1.0 / 1.3004).abs() < 1e-12);
        assert!((o.market_price(o.internal_limit()) - 1.3004).abs() < 1e-9);
    }

    #[test]
    fn terms_round_against_the_signer_on_the_derived_side() {
        let sell = ticket(TradeSide::Sell, TradeKind::Limit, TradeUnit::Base, "1000", Some(1.3004), TradeTif::Gtc);
        let o = order(&sell);
        let tm = terms(o, o.internal_limit());
        assert_eq!(tm.pay, 1000.0);
        assert_eq!(tm.receive, 1300.4);
        let buy = ticket(TradeSide::Buy, TradeKind::Limit, TradeUnit::Base, "1000", Some(1.3004), TradeTif::Gtc);
        let o = order(&buy);
        let tm = terms(o, o.internal_limit());
        assert_eq!(tm.receive, 1000.0);
        assert!(tm.pay >= 1300.4 && tm.pay < 1300.400001, "{}", tm.pay);
        let spend = ticket(TradeSide::Buy, TradeKind::Limit, TradeUnit::Quote, "1300.4", Some(1.3004), TradeTif::Gtc);
        let o = order(&spend);
        let tm = terms(o, o.internal_limit());
        assert_eq!(tm.pay, 1300.4);
        assert!(tm.receive <= 1000.0 && tm.receive > 999.999998, "{}", tm.receive);
        let earn = ticket(TradeSide::Sell, TradeKind::Limit, TradeUnit::Quote, "1300.4", Some(1.3004), TradeTif::Gtc);
        let o = order(&earn);
        let tm = terms(o, o.internal_limit());
        assert_eq!(tm.receive, 1300.4);
        assert!(tm.pay >= 1000.0 && tm.pay < 1000.000001, "{}", tm.pay);
    }

    #[test]
    fn the_flags_carry_the_tif_and_the_paid_side_carries_tfsell() {
        let f = |side, kind, unit, tif| flags(order(&ticket(side, kind, unit, "1", Some(1.3), tif)));
        assert_eq!(f(TradeSide::Sell, TradeKind::Limit, TradeUnit::Base, TradeTif::Gtc), vec!["tfSell".to_string()]);
        assert_eq!(
            f(TradeSide::Sell, TradeKind::Limit, TradeUnit::Base, TradeTif::Ioc),
            vec!["tfImmediateOrCancel".to_string(), "tfSell".to_string()]
        );
        assert_eq!(f(TradeSide::Buy, TradeKind::Limit, TradeUnit::Base, TradeTif::Fok), vec!["tfFillOrKill".to_string()]);
        assert_eq!(f(TradeSide::Buy, TradeKind::Market, TradeUnit::Base, TradeTif::Gtc), vec!["tfImmediateOrCancel".to_string()]);
        assert_eq!(
            f(TradeSide::Buy, TradeKind::Market, TradeUnit::Quote, TradeTif::Gtc),
            vec!["tfImmediateOrCancel".to_string(), "tfSell".to_string()]
        );
        assert_eq!(f(TradeSide::Sell, TradeKind::Limit, TradeUnit::Quote, TradeTif::Gtc), Vec::<String>::new());
    }

    #[test]
    fn each_side_and_unit_signs_its_own_amounts_and_flags() {
        let cases: [(TradeSide, TradeUnit, &str, (&str, &str), (&str, &str), &[&str]); 4] = [
            (TradeSide::Sell, TradeUnit::Base, "100", ("XRP", "100"), ("RLUSD", "130"), &["tfSell"]),
            (TradeSide::Buy, TradeUnit::Base, "100", ("RLUSD", "130"), ("XRP", "100"), &[]),
            (TradeSide::Sell, TradeUnit::Quote, "130", ("XRP", "100"), ("RLUSD", "130"), &[]),
            (TradeSide::Buy, TradeUnit::Quote, "130", ("RLUSD", "130"), ("XRP", "100"), &["tfSell"]),
        ];
        for (side, unit, amount, gets, pays, want_flags) in cases {
            let q = compute(&ticket(side, TradeKind::Limit, unit, amount, Some(1.3), TradeTif::Gtc));
            assert_eq!((q.pay_asset.as_str(), q.pay.as_deref().unwrap_or("")), gets, "TakerGets for {side:?} {unit:?}");
            assert_eq!((q.receive_asset.as_str(), q.receive.as_deref().unwrap_or("")), pays, "TakerPays for {side:?} {unit:?}");
            assert_eq!(q.flags, want_flags.iter().map(|f| f.to_string()).collect::<Vec<_>>(), "flags for {side:?} {unit:?}");
            assert!(q.lockable, "{side:?} {unit:?}");
        }
    }

    #[test]
    fn spending_a_whole_token_balance_signs_the_ledgers_string() {
        CHANNEL.set_token("WHOLETEST", (0.123456789, true, None, "0.123456789".to_string()));
        let mut t = ticket(TradeSide::Buy, TradeKind::Limit, TradeUnit::Quote, "0.123456789", Some(2.0), TradeTif::Ioc);
        t.quote = "WHOLETEST".to_string();
        let q = compute(&t);
        assert_eq!(q.pay_asset, "WHOLETEST");
        assert_eq!(q.pay.as_deref(), Some("0.123456789"));
        assert_eq!(q.receive.as_deref(), Some("0.061728"));
        assert_eq!(q.flags, vec!["tfImmediateOrCancel".to_string(), "tfSell".to_string()]);
        assert_eq!(q.max_pay, Some(0.123456789));
        assert_eq!(q.max, Some(0.123456789));
        t.unit = TradeUnit::Base;
        t.amount = "0.061728".to_string();
        let q = compute(&t);
        assert_ne!(q.pay.as_deref(), Some("0.123456789"), "a typed XRP figure derives the paid side");
        assert_eq!(q.max_pay, Some(0.123456789));
    }

    #[test]
    fn a_ticket_with_nothing_picked_quotes_only_the_book() {
        let mut t = ticket(TradeSide::Sell, TradeKind::Limit, TradeUnit::Base, "100", Some(1.3), TradeTif::Gtc);
        t.side = None;
        t.kind = None;
        let q = compute(&t);
        assert!(q.market_ok);
        assert_eq!(q.pay_asset, "");
        assert!(q.pay.is_none() && q.receive.is_none() && q.flags.is_empty() && q.max.is_none() && q.max_pay.is_none());
        assert!(!q.lockable);
        t.side = Some(TradeSide::Sell);
        let q = compute(&t);
        assert_eq!((q.pay_asset.as_str(), q.receive_asset.as_str()), ("XRP", "RLUSD"));
        assert!(!q.lockable, "Review waits for the kind");
        t.side = None;
        t.kind = Some(TradeKind::Limit);
        assert!(!compute(&t).lockable, "Review waits for the side");
    }

    #[test]
    fn a_market_order_far_from_mid_on_an_unstable_book_warns_and_still_locks() {
        const LEDGER: u64 = 900_000_000;
        CHANNEL.xrp_node_tx.send_modify(|n| n.ledger_index = Some(LEDGER));
        for code in ["OFFTEST", "NEARTEST"] {
            CHANNEL.orderbook_tx.send_modify(|books| {
                books.insert(
                    format!("XRP/{code}"),
                    OrderBook {
                        ledger: LEDGER,
                        bids: vec![(1.0, 1000.0)],
                        asks: vec![(1.1, 1000.0)],
                        amm: None,
                        require_auth: None,
                        global_freeze: None,
                        tick_size: None,
                    },
                );
            });
        }
        let summary = |mid: f64, holes: u32| MarketSummary { ledger: LEDGER, mid, spread_pct: 10.0, holes, ..Default::default() };
        CHANNEL.markets_tx.send_modify(|m| {
            m.insert("XRP/OFFTEST".to_string(), summary(2.0, 5));
            m.insert("XRP/NEARTEST".to_string(), summary(1.0, 0));
        });

        let mut t = ticket(TradeSide::Sell, TradeKind::Market, TradeUnit::Base, "10", None, TradeTif::Gtc);
        t.quote = "OFFTEST".to_string();
        let q = compute(&t);
        assert!(
            matches!(q.verdict, Verdict::Walked { off_market: true, unstable: true, bound_pct, .. } if bound_pct < -BOUND_GUARD_PCT),
            "{:?}",
            q.verdict
        );
        assert!(q.lockable, "an off-market walk no longer refuses the order");
        assert_eq!(q.pay.as_deref(), Some("10"));
        assert_eq!(q.receive.as_deref(), Some("9.999999"), "the signed bound is still the walk's floor");
        assert_eq!(q.flags, vec!["tfImmediateOrCancel".to_string(), "tfSell".to_string()]);

        t.quote = "NEARTEST".to_string();
        let q = compute(&t);
        assert!(matches!(q.verdict, Verdict::Walked { off_market: false, unstable: false, .. }), "{:?}", q.verdict);
        assert!(q.lockable);

        t.quote = "OFFTEST".to_string();
        t.side = Some(TradeSide::Buy);
        t.unit = TradeUnit::Quote;
        t.amount = "11".to_string();
        let q = compute(&t);
        assert_eq!(q.pay.as_deref(), Some("11"), "spending the quote pays exactly what was typed");
        assert_eq!((q.pay_asset.as_str(), q.receive_asset.as_str()), ("OFFTEST", "XRP"));
        assert!(q.flags.contains(&"tfSell".to_string()));
        assert!(q.counter.is_some_and(|c| (c - 10.0).abs() < 1e-9), "{:?}", q.counter);
    }

    #[test]
    fn only_a_resting_limit_holds_the_reserve() {
        let rest = |kind, tif| can_rest(order(&ticket(TradeSide::Sell, kind, TradeUnit::Base, "1", Some(1.3), tif)));
        assert!(rest(TradeKind::Limit, TradeTif::Gtc));
        assert!(!rest(TradeKind::Limit, TradeTif::Ioc));
        assert!(!rest(TradeKind::Limit, TradeTif::Fok));
        assert!(!rest(TradeKind::Market, TradeTif::Gtc));
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
        let json = r#"{"base":"XRP","quote":"RLUSD","side":"sell","kind":"limit","unit":"base","amount":"250","limit":1.3004,"tif":"gtc"}"#;
        let t: TradeTicket = serde_json::from_str(json).unwrap();
        assert_eq!(t.side, Some(TradeSide::Sell));
        assert_eq!(t.kind, Some(TradeKind::Limit));
        assert_eq!(t.unit, TradeUnit::Base);
        assert_eq!(t.tif, TradeTif::Gtc);
        assert_eq!(t.limit, Some(1.3004));
        let market = r#"{"base":"XRP","quote":"RLUSD","side":"buy","kind":"market","unit":"quote","amount":"","limit":null,"tif":"gtc"}"#;
        let t: TradeTicket = serde_json::from_str(market).unwrap();
        assert_eq!(t.kind, Some(TradeKind::Market));
        assert_eq!(t.unit, TradeUnit::Quote);
        assert_eq!(t.limit, None);
        let fresh = r#"{"base":"XRP","quote":"RLUSD","side":null,"kind":null,"unit":"base","amount":"","limit":null,"tif":"gtc"}"#;
        let t: TradeTicket = serde_json::from_str(fresh).unwrap();
        assert_eq!((t.side, t.kind), (None, None));
    }

    #[test]
    fn the_wire_strings_trim_as_desktops() {
        assert_eq!(amount_string(1300.4), "1300.4");
        assert_eq!(amount_string(250.0), "250");
        assert_eq!(amount_string(floor_to(0.1234567, SIGN_PLACES)), "0.123456");
    }
}
