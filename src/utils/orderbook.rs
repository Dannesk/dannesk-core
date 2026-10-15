

use crate::channel::{OrderBook, CHANNEL};


pub const DEGENERATE_SPREAD_PCT: f64 = 5.0;


pub fn resolve(base: &str, quote: &str) -> Option<(String, OrderBook, bool)> {
    if let Some((pair, book)) = CHANNEL.book(base, quote) {
        return Some((pair, with_amm(&book), false));
    }
    if base == "XRP" || quote == "XRP" || base == quote {
        return None;
    }
    let (_, base_leg) = CHANNEL.book("XRP", base)?;
    let (_, quote_leg) = CHANNEL.book("XRP", quote)?;
    let base_leg = with_amm(&base_leg);
    let quote_leg = with_amm(&quote_leg);

    let asks = combine(&quote_leg.asks, &base_leg.bids);
    let bids = combine(&quote_leg.bids, &base_leg.asks);

    Some((
        format!("{base}/{quote}"),
        OrderBook {
            ledger: base_leg.ledger.min(quote_leg.ledger),
            bids,
            asks,
      
            amm: None,
           
            require_auth: worse(base_leg.require_auth, quote_leg.require_auth),
            global_freeze: worse(base_leg.global_freeze, quote_leg.global_freeze),
         
            tick_size: match (base_leg.tick_size, quote_leg.tick_size) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (t, None) | (None, t) => t,
            },
        },
        true,
    ))
}

const AMM_SLICES: usize = 40;


pub fn with_amm(book: &OrderBook) -> OrderBook {
    let Some(pool) = book.amm.filter(|a| a.xrp > 0.0 && a.token > 0.0 && a.fee_pct >= 0.0 && a.fee_pct < 100.0) else {
        return book.clone();
    };
    let f = pool.fee_pct / 100.0;
    let (x, y) = (pool.xrp, pool.token);
    let spot = y / x;
    let tick = display_tick(spot);

    let q_bid = spot * (1.0 - f);
    let depth_bid = |p: f64| ((x * y * (1.0 - f) / p).sqrt() - x) / (1.0 - f);
    let mut bid_slices: Vec<(f64, f64)> = Vec::with_capacity(AMM_SLICES);
    let mut prev = 0.0;
    let mut edge = (q_bid / tick - 1e-9).floor() * tick;
    for _ in 0..AMM_SLICES {
        if edge <= 0.0 { break; }
        let d = depth_bid(edge);
        if d - prev > 0.0 {
            bid_slices.push((edge, d - prev));
        }
        prev = d;
        edge -= tick;
    }

    let q_ask = spot / (1.0 - f);
    let depth_ask = |p: f64| x * (1.0 - (y / (x * p * (1.0 - f))).sqrt());
    let mut ask_slices: Vec<(f64, f64)> = Vec::with_capacity(AMM_SLICES);
    let mut prev = 0.0;
    let mut edge = (q_ask / tick + 1e-9).ceil() * tick;
    for _ in 0..AMM_SLICES {
        let d = depth_ask(edge);
        if d - prev > 0.0 {
            ask_slices.push((edge, d - prev));
        }
        prev = d;
        edge += tick;
    }

    OrderBook {
        ledger: book.ledger,
        bids: merge_side(&book.bids, &bid_slices, false),
        asks: merge_side(&book.asks, &ask_slices, true),
        amm: book.amm,
        require_auth: book.require_auth,
        global_freeze: book.global_freeze,
        tick_size: book.tick_size,
    }
}

fn worse(a: Option<bool>, b: Option<bool>) -> Option<bool> {
    match (a, b) {
        (Some(true), _) | (_, Some(true)) => Some(true),
        (Some(false), _) | (_, Some(false)) => Some(false),
        _ => None,
    }
}

fn merge_side(a: &[(f64, f64)], b: &[(f64, f64)], asks: bool) -> Vec<(f64, f64)> {
    let better = |p: f64, q: f64| if asks { p < q } else { p > q };
    let mut out: Vec<(f64, f64)> = Vec::with_capacity(a.len() + b.len());
    let (mut i, mut j) = (0, 0);
    while i < a.len() || j < b.len() {
        let next = match (a.get(i), b.get(j)) {
            (Some(&l), Some(&r)) => {
                if better(l.0, r.0) || l.0 == r.0 { i += 1; l } else { j += 1; r }
            }
            (Some(&l), None) => { i += 1; l }
            (None, Some(&r)) => { j += 1; r }
            (None, None) => break,
        };
        match out.last_mut() {
            Some(last) if last.0 == next.0 => last.1 += next.1,
            _ => out.push(next),
        }
    }
    out
}

pub fn display_tick(p: f64) -> f64 {
    if p >= 100.0 { 0.01 } else if p >= 1.0 { 0.0001 } else { 0.00001 }
}


pub fn display_levels(levels: &[(f64, f64)], asks: bool, max: usize) -> Vec<(f64, f64, f64)> {
    let mut out: Vec<(f64, f64, f64)> = Vec::with_capacity(max);
    let mut cumulative = 0.0;
    for &(p, a) in levels {
        if p <= 0.0 || a <= 0.0 {
            continue;
        }
        let tick = display_tick(p);
        let bucket = if asks { (p / tick - 1e-9).ceil() * tick } else { (p / tick + 1e-9).floor() * tick };
        cumulative += a;
        match out.last_mut() {
            Some(last) if last.0 == bucket => {
                last.1 += a;
                last.2 = cumulative;
            }
            _ => {
                if out.len() == max {
                    break;
                }
                out.push((bucket, a, cumulative));
            }
        }
    }
    out
}


pub fn oriented(base: &str, quote: &str) -> Option<(OrderBook, bool)> {
    let (pair, book, via) = resolve(base, quote)?;
    if pair == format!("{base}/{quote}") {
        return Some((book, via));
    }
    let flip = |levels: &[(f64, f64)]| {
        levels
            .iter()
            .filter(|&&(p, _)| p > 0.0)
            .map(|&(p, a)| (1.0 / p, a * p))
            .collect::<Vec<_>>()
    };
    Some((
        OrderBook {
            ledger: book.ledger,
            bids: flip(&book.asks),
            asks: flip(&book.bids),
            amm: book.amm,
            require_auth: book.require_auth,
            global_freeze: book.global_freeze,
            tick_size: book.tick_size,
        },
        via,
    ))
}

pub fn fmt_price(p: f64) -> String {
    if p >= 100.0 { format!("{p:.2}") } else if p >= 1.0 { format!("{p:.4}") } else { format!("{p:.5}") }
}


pub fn touch(book: &OrderBook) -> Option<f64> {
    book.bids.first().map(|l| l.0).filter(|&p| p > 0.0)
}

pub fn mid_and_spread(book: &OrderBook) -> Option<(f64, f64)> {
    let s = spread_pct(book)?;
    Some(((book.bids[0].0 + book.asks[0].0) / 2.0, s))
}


pub fn spread_pct(book: &OrderBook) -> Option<f64> {
    let bb = book.bids.first().map(|l| l.0).filter(|&p| p > 0.0)?;
    let ba = book.asks.first().map(|l| l.0).filter(|&p| p > 0.0)?;
    Some((ba - bb) / ((bb + ba) / 2.0) * 100.0)
}


#[derive(Debug, Clone, Copy)]
pub struct Quote {
  
    pub vwap: f64,
 
    pub floor: f64,
    pub filled: f64,
    pub receive: f64,

    pub depth_short: bool,

    pub ledger: u64,
  
    pub from_pool: f64,
    pub clob_levels: usize,
}

pub fn walk(pay: &str, recv: &str, amount: f64, anchor_is_pay: bool) -> Option<Quote> {
    if amount <= 0.0 || !amount.is_finite() {
        return None;
    }
    let (token, pay_is_xrp) = match (pay, recv) {
        ("XRP", t) if t != "XRP" => (t, true),
        (t, "XRP") if t != "XRP" => (t, false),
        _ => return None,
    };
  
    let (_, book) = CHANNEL.book("XRP", token)?;

  
    let levels: Vec<(f64, f64)> = if pay_is_xrp {
        book.bids.iter().filter(|&&(p, a)| p > 0.0 && a > 0.0).map(|&(p, a)| (p, a)).collect()
    } else {
        book.asks.iter().filter(|&&(p, a)| p > 0.0 && a > 0.0).map(|&(p, a)| (1.0 / p, a * p)).collect()
    };

 
    let pool = book.amm.filter(|p| p.xrp > 0.0 && p.token > 0.0 && p.fee_pct >= 0.0 && p.fee_pct < 100.0);
    let (a0, b0, g) = match pool {
        Some(p) => {
            let g = 1.0 - p.fee_pct / 100.0;
            if pay_is_xrp { (p.xrp, p.token, g) } else { (p.token, p.xrp, g) }
        }
        None => (0.0, 0.0, 1.0),
    };
    let has_pool = pool.is_some() && g > 0.0;
 
    let out_for = |d: f64| b0 * d * g / (a0 + d * g);
    let depth_to = |p: f64| ((a0 * b0 * g / p).sqrt() - a0) / g;

    let mut spent = 0.0;
    let mut got = 0.0;
    let mut from_pool = 0.0;
    let mut worst = f64::INFINITY;
    let mut ci = 0usize;
    let mut clob_levels = 0usize;
    let mut last_touched: Option<usize> = None;

    for _ in 0..4096 {
        let left = if anchor_is_pay { amount - spent } else { amount - got };
        if left <= 1e-12 {
            break;
        }
        let pool_rate = if has_pool {
            let an = a0 + from_pool;
            let bn = b0 - out_for(from_pool);
            if an > 0.0 && bn > 0.0 { bn / an * g } else { 0.0 }
        } else {
            0.0
        };
        let clob_rate = levels.get(ci).map(|l| l.0).unwrap_or(0.0);
        if pool_rate <= 0.0 && clob_rate <= 0.0 {
            break; 
        }

        if pool_rate > clob_rate {
         
            let room = if clob_rate > 0.0 {
                (depth_to(clob_rate) - from_pool).max(0.0)
            } else {
                f64::INFINITY
            };
            let want = if anchor_is_pay {
                left
            } else {
            
                let bn = b0 - out_for(from_pool);
                if left >= bn { f64::INFINITY } else { (a0 + from_pool) * left / (g * (bn - left)) }
            };
            let take = want.min(room);
            if !(take > 0.0) {
            
                if clob_rate > 0.0 { ci += 1; continue; }
                break;
            }
            if !take.is_finite() {
                break;
            }
            let out = out_for(from_pool + take) - out_for(from_pool);
            if out <= 0.0 {
                break;
            }
            from_pool += take;
            spent += take;
            got += out;
            let an = a0 + from_pool;
            let bn = b0 - out_for(from_pool);
            if an > 0.0 && bn > 0.0 {
                worst = worst.min(bn / an * g);
            }
        } else {
            let (rate, cap) = levels[ci];
            let want = if anchor_is_pay { left } else { left / rate };
            let take = want.min(cap);
            if take <= 0.0 {
                ci += 1;
                continue;
            }
            spent += take;
            got += take * rate;
            worst = worst.min(rate);
            if last_touched != Some(ci) {
                last_touched = Some(ci);
                clob_levels += 1;
            }
            if take >= cap - 1e-12 {
                ci += 1;
            }
        }
    }

    if spent <= 0.0 || got <= 0.0 || !worst.is_finite() {
        return None;
    }
    let vwap = got / spent;
  
    let floor = floor_sig(worst.min(vwap), 9);
    let target = if anchor_is_pay { spent } else { got };
    Some(Quote {
        vwap,
        floor,
        filled: spent,
        receive: got,
        depth_short: target < amount - amount.abs().max(1.0) * 1e-9,
        ledger: book.ledger,
        from_pool,
        clob_levels,
    })
}


fn snap(s: f64) -> f64 {
    let r = s.round();
    if (s - r).abs() <= 4.0 * f64::EPSILON * r.abs().max(1.0) { r } else { s }
}


pub fn floor_to(x: f64, places: usize) -> f64 {
    let k = 10f64.powi(places as i32);
    snap(x * k).floor() / k
}


pub fn floor_sig(x: f64, digits: i32) -> f64 {
    if x <= 0.0 || !x.is_finite() {
        return x;
    }
    let factor = 10f64.powf((digits - 1) as f64 - x.abs().log10().floor());
    (x * factor).floor() / factor
}


pub fn ceil_to(x: f64, places: usize) -> f64 {
    let k = 10f64.powi(places as i32);
    snap(x * k).ceil() / k
}


fn combine(quote_levels: &[(f64, f64)], base_levels: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    let (mut i, mut j) = (0usize, 0usize);
    let mut q_left = quote_levels.first().map(|l| l.1).unwrap_or(0.0);
    let mut b_left = base_levels.first().map(|l| l.1).unwrap_or(0.0);

    while i < quote_levels.len() && j < base_levels.len() && out.len() < 15 {
        let q_price = quote_levels[i].0;
        let b_price = base_levels[j].0;
        if q_price <= 0.0 || b_price <= 0.0 {
            break;
        }
        let xrp = q_left.min(b_left);
        if xrp > 0.0 {
            out.push((q_price / b_price, xrp * b_price));
        }
        q_left -= xrp;
        b_left -= xrp;
        if q_left <= 0.0 {
            i += 1;
            if i < quote_levels.len() { q_left = quote_levels[i].1; }
        }
        if b_left <= 0.0 {
            j += 1;
            if j < base_levels.len() { b_left = base_levels[j].1; }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel::Amm;

    fn pool() -> Amm {
        Amm { xrp: 1_679_427.563204, token: 2_314_144.269111297, fee_pct: 0.205 }
    }

    fn book() -> OrderBook {
        OrderBook {
            ledger: 1,
            bids: vec![(1.37502, 305.0), (1.3749, 47.0)],
            asks: vec![(1.37516, 89.0), (1.3757, 471.0)],
            amm: Some(pool()),
            require_auth: Some(false),
            global_freeze: Some(false),
            tick_size: None,
        }
    }

    #[test]
    fn the_constant_product_matches_the_engine() {
        let p = pool();
        let f = p.fee_pct / 100.0;
        let dx = 3033.887481 * (1.0 - f);
        let out = p.token * dx / (p.xrp + dx);
        assert!((out - 4164.3948).abs() / 4164.3948 < 1e-4, "{out}");
    }

    #[test]
    fn the_pool_becomes_levels_best_first_that_sum_to_its_depth() {
        let b = with_amm(&book());

        assert!(b.bids[0].0 > 1.37502 && b.bids[0].0 <= 1.37511, "{:?}", b.bids[0]);
        assert!(b.bids.windows(2).all(|w| w[0].0 > w[1].0 && w[1].1 > 0.0));
        assert!(b.asks.windows(2).all(|w| w[0].0 < w[1].0 && w[1].1 > 0.0));
        
        let p = pool();
        let f = p.fee_pct / 100.0;
        let deepest = b.bids.last().unwrap().0;
        let expect = ((p.xrp * p.token * (1.0 - f) / deepest).sqrt() - p.xrp) / (1.0 - f);
        let amm_only: f64 = b.bids.iter().map(|l| l.1).sum::<f64>() - 305.0 - 47.0;
        assert!((amm_only - expect).abs() / expect < 1e-6, "{amm_only} vs {expect}");
        assert!(b.amm.is_some());
    }


    #[test]
    fn an_unanswered_issuer_is_permissive() {
        assert_eq!(worse(None, None), None);
        assert_eq!(worse(Some(false), None), Some(false));
        assert_eq!(worse(None, Some(false)), Some(false));
        // A bridge is only as tradeable as its worst leg.
        assert_eq!(worse(Some(true), Some(false)), Some(true));
        assert_eq!(worse(Some(false), Some(true)), Some(true));
        assert_eq!(worse(Some(true), None), Some(true));
    }

    #[test]
    fn a_book_without_a_pool_is_untouched() {
        let mut b = book();
        b.amm = None;
        let out = with_amm(&b);
        assert_eq!(out.bids, b.bids);
        assert_eq!(out.asks, b.asks);
    }

   
    #[test]
    fn display_buckets_merge_and_accumulate() {
        let bids = vec![(1.42259999, 10.0), (1.42251, 5.0), (1.4224, 1.0)];
        let d = display_levels(&bids, false, 8);
        assert_eq!(d.len(), 2);
        assert!((d[0].0 - 1.4225).abs() < 1e-9 && (d[0].1 - 15.0).abs() < 1e-9 && (d[0].2 - 15.0).abs() < 1e-9);
        assert!((d[1].0 - 1.4224).abs() < 1e-9 && (d[1].2 - 16.0).abs() < 1e-9);
        let asks = vec![(1.42250001, 2.0), (1.4226, 3.0)];
        let d = display_levels(&asks, true, 8);
        assert!((d[0].0 - 1.4226).abs() < 1e-9 && (d[0].1 - 5.0).abs() < 1e-9);
        assert_eq!(display_levels(&asks, true, 1).len(), 1);
    }

  
    #[test]
    fn the_pool_is_consumed_only_down_to_the_next_level() {
        let p = pool();
        let g = 1.0 - p.fee_pct / 100.0;
        let (x, y) = (p.xrp, p.token);
        let depth_to = |q: f64| ((x * y * g / q).sqrt() - x) / g;
        let to_book = depth_to(1.37502);
        assert!(to_book > 0.0, "pool touch must beat the book's 1.37502");
        assert!(to_book < x * 0.01, "{to_book} should be a thin top slice");
        assert!(depth_to(1.3749) > to_book);
    }

    #[test]
    fn the_floor_never_sits_above_the_walk() {
        for r in [1.37511_f64, 0.000123456789, 98765.4321, 1.0] {
            assert!(floor_sig(r, 9) <= r, "{r}");
        }
        assert_eq!(floor_sig(1.234567891234, 9), 1.23456789);
        assert!(floor_sig(1.4455000001, 9) <= 1.4455000001);
    }

    #[test]
    fn rounding_goes_the_way_the_bound_points() {
        assert_eq!(floor_to(356.2499, 2), 356.24);
        assert_eq!(ceil_to(703.1854, 2), 703.19);
        assert_eq!(ceil_to(703.19, 2), 703.19);
        assert_eq!(floor_to(250.0, 6), 250.0);
    }

 
    #[test]
    fn a_typed_amount_survives_the_round_trip_to_the_wire() {
        assert_eq!(floor_to(4100.4, 6), 4100.4);
        assert_eq!(floor_to(4260.48, 6), 4260.48);
        assert_eq!(floor_to(1.000001, 6), 1.000001);
        assert_eq!(ceil_to(4260.48, 6), 4260.48);
        assert_eq!(ceil_to(4100.4, 6), 4100.4);
        assert_eq!(floor_to(0.1234567, 6), 0.123456);
        assert_eq!(ceil_to(0.1234561, 6), 0.123457);
    }
}
