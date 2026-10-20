

use crate::channel::CHANNEL;
use crate::ws::RatesCommand;
use serde_json::Value;
use std::collections::HashSet;

/// Updates the want-set; true when it changed, the only time bookd needs telling.
pub fn track(books: &mut HashSet<String>, cmd: &RatesCommand) -> bool {
    match cmd {
        RatesCommand::SubscribeBook(pair) => books.insert(pair.clone()),
        RatesCommand::UnsubscribeBook(pair) => books.remove(pair),
    }
}

pub fn book_pairs() -> Vec<String> {
    crate::utils::tokens::TOKENS.iter().map(|t| format!("XRP/{}", t.code)).collect()
}

pub fn subscribe_frame(pair: &str, on: bool) -> String {
    serde_json::json!({ "command": if on { "subscribe" } else { "unsubscribe" }, "orderbook": pair }).to_string()
}

pub fn history_frame(assets: &[String]) -> String {
    serde_json::json!({ "command": "history", "assets": assets }).to_string()
}


pub fn history_assets() -> Vec<String> {
    let mut want: Vec<String> = vec!["BTC".into(), "XRP".into()];
    want.extend(crate::utils::tokens::TOKENS.iter().map(|t| t.rate_key.to_string()));
    want.retain(|k| k != "USD");
    want.sort();
    want.dedup();
    want
}

pub fn process_message(text: &str) {
    let Ok(data) = serde_json::from_str::<Value>(text) else { return };

    match data.get("type").and_then(|v| v.as_str()) {
        Some("history") => {
            let Some(symbol) = data.get("symbol").and_then(|v| v.as_str()) else { return };
            let short = parse_history_vec(data.get("short"));
            let mut history = CHANNEL.rate_history_tx.borrow().clone();
            history.insert(symbol.to_string(), short);
            let _ = CHANNEL.rate_history_tx.send(history);

            let long = parse_history_vec(data.get("long"));
            let mut history_long = CHANNEL.rate_history_long_tx.borrow().clone();
            history_long.insert(symbol.to_string(), long);
            let _ = CHANNEL.rate_history_long_tx.send(history_long);
        }
        Some("orderbook") => {
          
            let Some(pair) = data.get("pair").and_then(|v| v.as_str()) else { return };
            let amm = data.get("amm").filter(|a| a.is_object()).and_then(|a| {
                Some(crate::channel::Amm {
                    xrp: a.get("xrp")?.as_f64()?,
                    token: a.get("token")?.as_f64()?,
                    fee_pct: a.get("fee_pct")?.as_f64()?,
                })
            });
            let book = crate::channel::OrderBook {
                ledger: data.get("ledger").and_then(|v| v.as_u64()).unwrap_or(0),
                bids: parse_book_levels(data.get("bids")),
                asks: parse_book_levels(data.get("asks")),
                amm,
                require_auth: data.get("require_auth").and_then(|v| v.as_bool()),
                global_freeze: data.get("global_freeze").and_then(|v| v.as_bool()),
                tick_size: data.get("tick_size").and_then(|v| v.as_u64()).map(|t| t as u32),
            };
            CHANNEL.orderbook_tx.send_modify(|books| {
                books.insert(pair.to_string(), book);
            });
        }
        Some("markets") => {
          
            let Some(pairs) = data.get("pairs").and_then(|v| v.as_object()) else { return };
            let f = |m: &Value, k: &str| m.get(k).and_then(|v| v.as_f64()).unwrap_or(0.0);
            let parsed = pairs
                .iter()
                .map(|(pair, m)| {
                    (
                        pair.clone(),
                        crate::channel::MarketSummary {
                            ledger: m.get("ledger").and_then(|v| v.as_u64()).unwrap_or(0),
                            mid: f(m, "mid"),
                            spread_pct: f(m, "spread_pct"),
                            bid1: f(m, "bid1"),
                            ask1: f(m, "ask1"),
                            bid5: f(m, "bid5"),
                            ask5: f(m, "ask5"),
                            amm_xrp: f(m, "amm_xrp"),
                            amm1: f(m, "amm1"),
                            win_n: m.get("win_n").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
                            win_mid: f(m, "win_mid"),
                            win_spread_pct: f(m, "win_spread_pct"),
                            win_bid1: f(m, "win_bid1"),
                            win_ask1: f(m, "win_ask1"),
                            win_bid5: f(m, "win_bid5"),
                            win_ask5: f(m, "win_ask5"),
                            win_amm1: f(m, "win_amm1"),
                            holes: m.get("holes").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
                        },
                    )
                })
                .collect();
            let _ = CHANNEL.markets_tx.send(parsed);
        }
        Some("status") => crate::ws::apply_status_frame(&data),
        Some("rates") => {
            let Some(updates) = data.get("updates").and_then(|v| v.as_array()) else { return };
            CHANNEL.rates_tx.send_modify(|rates| {
                for update in updates {
                    if let Some((asset, rate)) = parse_asset_price(update) {
                        rates.insert(asset, rate);
                    }
                }
            });
        }
        _ => {
            if let Some((asset, rate)) = parse_asset_price(&data) {
                CHANNEL.rates_tx.send_modify(|rates| { rates.insert(asset, rate); });
            }
        }
    }
}


fn parse_asset_price(entry: &Value) -> Option<(String, f32)> {
    let symbol = entry.get("symbol").and_then(|v| v.as_str())?;
    let price = entry.get("price").and_then(|v| v.as_str())?;
    let rate = price.parse::<f32>().ok()?;
    Some((symbol.to_string(), rate))
}

fn parse_book_levels(v: Option<&Value>) -> Vec<(f64, f64)> {
    v.and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter().filter_map(|entry| {
                let level = entry.as_array()?;
                Some((level.first()?.as_f64()?, level.get(1)?.as_f64()?))
            }).collect()
        })
        .unwrap_or_default()
}

fn parse_history_vec(v: Option<&Value>) -> Vec<(u64, f32)> {
    v.and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter().filter_map(|entry| {
                let arr = entry.as_array()?;
                let ts = arr.first()?.as_u64()?;
                let price = arr.get(1)?.as_f64()? as f32;
                Some((ts, price))
            }).collect()
        })
        .unwrap_or_default()
}
