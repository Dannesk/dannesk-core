

use crate::channel::{MarketSummary, CHANNEL};

/// XRP within 1% of mid, per side, for a market to read as live.
pub const LIVE_XRP: f64 = 5_000.0;
/// XRP within 5% of mid, per side, for a market to read as thin rather than
/// absent.
pub const THIN_XRP: f64 = 500.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Depth {
    Live,
    Thin,
    None,
}

impl Depth {
    pub fn min(self, other: Depth) -> Depth {
        use Depth::*;
        match (self, other) {
            (None, _) | (_, None) => None,
            (Thin, _) | (_, Thin) => Thin,
            _ => Live,
        }
    }
}


#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Market {
    pub depth: Depth,
    pub mid: f64,
    pub spread_pct: f64,
    
    pub walkable_xrp: f64,
  
    pub stable: bool,
    pub holes: u32,
}


pub const STABLE_HOLES: u32 = 1;


pub fn eff(s: &MarketSummary) -> MarketSummary {
    if s.win_n == 0 {
        return *s;
    }
    MarketSummary {
        mid: s.win_mid,
        spread_pct: s.win_spread_pct,
        bid1: s.win_bid1,
        ask1: s.win_ask1,
        bid5: s.win_bid5,
        ask5: s.win_ask5,
        amm1: s.win_amm1,
        ..*s
    }
}

pub fn classify(s: &MarketSummary) -> Depth {
    let s = eff(s);
    if s.mid <= 0.0 {
        return Depth::None;
    }
    let (b1, a1) = (s.bid1 + s.amm1, s.ask1 + s.amm1);
    if b1 >= LIVE_XRP && a1 >= LIVE_XRP {
        return Depth::Live;
    }
    let (b5, a5) = (s.bid5 + s.amm1, s.ask5 + s.amm1);
    if b5 >= THIN_XRP && a5 >= THIN_XRP {
        return Depth::Thin;
    }
    Depth::None
}

pub fn walkable(s: &MarketSummary) -> f64 {
    let s = eff(s);
    (s.bid5 + s.amm1).min(s.ask5 + s.amm1)
}

pub fn stable(s: &MarketSummary) -> bool {
    s.holes <= STABLE_HOLES
}

pub fn leg(token: &str) -> Option<MarketSummary> {
    CHANNEL.markets_rx.borrow().get(&format!("XRP/{token}")).copied()
}


pub fn market(base: &str, quote: &str) -> Option<Market> {
    if base == quote {
        return None;
    }
    match (base, quote) {
        ("XRP", t) => {
            let raw = leg(t)?;
            let s = eff(&raw);
            Some(Market {
                depth: classify(&raw),
                mid: s.mid,
                spread_pct: s.spread_pct,
                walkable_xrp: walkable(&raw),
                stable: stable(&raw),
                holes: raw.holes,
            })
        }
        (t, "XRP") => {
            let raw = leg(t)?;
            let s = eff(&raw);
            Some(Market {
                depth: classify(&raw),
                mid: if s.mid > 0.0 { 1.0 / s.mid } else { 0.0 },
                spread_pct: s.spread_pct,
                walkable_xrp: walkable(&raw),
                stable: stable(&raw),
                holes: raw.holes,
            })
        }
        (b, q) => {
            let (rb, rq) = (leg(b)?, leg(q)?);
            let (lb, lq) = (eff(&rb), eff(&rq));
            let mid = if lb.mid > 0.0 && lq.mid > 0.0 { lq.mid / lb.mid } else { 0.0 };
            Some(Market {
                depth: classify(&rb).min(classify(&rq)),
                mid,
                spread_pct: lb.spread_pct + lq.spread_pct,
                walkable_xrp: walkable(&rb).min(walkable(&rq)),
                stable: stable(&rb) && stable(&rq),
                holes: rb.holes.max(rq.holes),
            })
        }
    }
}


pub fn label(base: &str, quote: &str) -> String {
    let Some(m) = market(base, quote) else { return "measuring\u{2026}".to_string() };
    let depth = match m.depth {
        Depth::Live => "liquid",
        Depth::Thin => "thin",
        Depth::None => "illiquid",
    };
    format!("{} \u{b7} {}", depth, if m.stable { "stable" } else { "unstable" })
}


pub fn healthy(base: &str, quote: &str) -> Option<bool> {
    market(base, quote).map(|m| m.depth == Depth::Live && m.stable)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(bid1: f64, ask1: f64, bid5: f64, ask5: f64, amm1: f64) -> MarketSummary {
        MarketSummary { ledger: 1, mid: 1.42, spread_pct: 0.11, bid1, ask1, bid5, ask5, amm_xrp: 0.0, amm1, ..Default::default() }
    }

   
    #[test]
    fn a_one_ledger_hole_does_not_demote_a_windowed_market() {
        let mut hole = s(0.0, 0.0, 0.0, 0.0, 5.8);
        hole.mid = 21.0;
        hole.win_n = 20;
        hole.win_mid = 2.005;
        hole.win_bid1 = 9990.0;
        hole.win_ask1 = 13480.0;
        hole.win_bid5 = 9990.0;
        hole.win_ask5 = 13480.0;
        hole.win_amm1 = 5.8;
        hole.holes = 2;
        assert_eq!(classify(&hole), Depth::Live);
        assert!(!stable(&hole), "two holes in twenty is a pattern");
        hole.holes = 1;
        assert!(stable(&hole));
        hole.win_n = 0;
        assert_eq!(classify(&hole), Depth::None);
    }

  
    #[test]
    fn the_measured_books_classify_as_decided() {
        assert_eq!(classify(&s(42_000.0, 41_000.0, 72_000.0, 99_000.0, 8_200.0)), Depth::Live);
        assert_eq!(classify(&s(10_000.0, 13_400.0, 10_000.0, 13_400.0, 5.0)), Depth::Live);
        assert_eq!(classify(&s(2_100.0, 2_700.0, 2_400.0, 8_400.0, 1_100.0)), Depth::Thin);
        assert_eq!(classify(&s(0.0, 0.0, 0.0, 0.0, 1.0)), Depth::None);
        let mut dust = s(0.0, 0.0, 0.0, 0.0, 0.0);
        dust.spread_pct = 0.05;
        assert_eq!(classify(&dust), Depth::None);
        let mut one_sided = s(50_000.0, 50_000.0, 50_000.0, 50_000.0, 0.0);
        one_sided.mid = 0.0;
        assert_eq!(classify(&one_sided), Depth::None);
    }

    
    #[test]
    fn a_pool_alone_can_be_live() {
        assert_eq!(classify(&s(0.0, 0.0, 0.0, 0.0, 6_000.0)), Depth::Live);
        assert_eq!(classify(&s(0.0, 0.0, 0.0, 0.0, 600.0)), Depth::Thin);
    }

    #[test]
    fn a_cross_takes_the_weakest_leg() {
        assert_eq!(Depth::Live.min(Depth::Thin), Depth::Thin);
        assert_eq!(Depth::Live.min(Depth::None), Depth::None);
        assert_eq!(Depth::Thin.min(Depth::None), Depth::None);
        assert_eq!(Depth::Live.min(Depth::Live), Depth::Live);
    }
}
