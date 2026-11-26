use dannesk_btc_codec::address::Address;
use dannesk_xrpl_codec::xaddress;

use crate::utils::tokens::{self, TOKENS};

const XRP_PLACES: usize = 6;
const XRP_DIGITS: usize = 15;
const BTC_PLACES: usize = 8;
const BTC_DIGITS: usize = 16;
const TAG_DIGITS: usize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Request<'a> {
    pub amount: &'a str,
    pub token: Option<&'a str>,
}

/// scanned or pasted XRP code, X-address keeps its tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XrpCode {
    pub to: String,
    pub tag: Option<u32>,
    pub amount: Option<String>,
    pub token: Option<&'static str>,
}

/// scanned or pasted, the amount in BTC.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BtcCode {
    pub to: String,
    pub amount: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeError {
    Refused,
    /// for non supported tokens.
    TokenNotSupported,
}


pub fn xrp_payload(classic: &str, tag: Option<u32>, request: Option<Request<'_>>) -> Option<String> {
    let address = match tag {
        Some(_) => xaddress::encode(classic, tag)?,
        None if xaddress::is_valid_classic_address(classic) => classic.to_string(),
        None => return None,
    };
    let Some(request) = request else { return Some(address) };
    let amount = plain_amount(request.amount, XRP_PLACES, XRP_DIGITS)?;
    let mut link = format!("xrpl://{address}?amount={amount}");
    if let Some(code) = request.token {
        let token = tokens::by_code(code)?;
        link.push_str(&format!("&currency={}&issuer={}", token.currency_hex, token.issuer));
    }
    Some(link)
}

pub fn btc_payload(address: &str, amount: Option<&str>) -> Option<String> {
    Address::parse(address).ok()?;
    let Some(amount) = amount else { return Some(address.to_string()) };
    Some(format!("bitcoin:{address}?amount={}", plain_amount(amount, BTC_PLACES, BTC_DIGITS)?))
}

pub fn read_xrp(text: &str) -> Result<XrpCode, CodeError> {
    let (address, query) = split(text, &["xrpl", "ripple"]).ok_or(CodeError::Refused)?;
    let resolved = xaddress::resolve(address).ok_or(CodeError::Refused)?;
    if query.get("network")?.is_some_and(|n| !n.eq_ignore_ascii_case("XRPL")) {
        return Err(CodeError::Refused);
    }
    let dt = query.get("dt")?;
    let alias = query.get("tag")?;
    if dt.is_some() && alias.is_some() && dt != alias {
        return Err(CodeError::Refused);
    }
    let tag = dt.or(alias).map(|t| tag_value(t).ok_or(CodeError::Refused)).transpose()?;
    if tag.is_some() && resolved.from_xaddress {
        return Err(CodeError::Refused);
    }
    let amount = query
        .get("amount")?
        .map(|a| plain_amount(a, XRP_PLACES, XRP_DIGITS).ok_or(CodeError::Refused))
        .transpose()?;
    let token = match (query.get("currency")?, query.get("issuer")?) {
        (None, None) => None,
        (Some(currency), Some(issuer)) => Some(
            TOKENS
                .iter()
                .find(|t| t.currency_hex.eq_ignore_ascii_case(currency) && t.issuer == issuer)
                .ok_or(CodeError::TokenNotSupported)?
                .code,
        ),
        _ => return Err(CodeError::Refused),
    };
    Ok(XrpCode { to: address.trim().to_string(), tag, amount, token })
}

pub fn read_btc(text: &str) -> Result<BtcCode, CodeError> {
    let (address, query) = split(text, &["bitcoin"]).ok_or(CodeError::Refused)?;
    let address = address.trim();
    if Address::parse(address).is_err() || query.keys().any(|k| k.starts_with("req-")) {
        return Err(CodeError::Refused);
    }
    let amount = query
        .get("amount")?
        .map(|a| plain_amount(a, BTC_PLACES, BTC_DIGITS).ok_or(CodeError::Refused))
        .transpose()?;
    Ok(BtcCode { to: address.to_string(), amount })
}

struct Query<'a>(Vec<(String, &'a str)>);

impl<'a> Query<'a> {
    /// refused when the key appears twice.
    fn get(&self, key: &str) -> Result<Option<&'a str>, CodeError> {
        let mut values = self.0.iter().filter(|(k, _)| k == key).map(|(_, v)| *v);
        let value = values.next();
        if values.next().is_some() {
            return Err(CodeError::Refused);
        }
        Ok(value)
    }

    fn keys(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(|(k, _)| k.as_str())
    }
}

fn split<'a>(text: &'a str, schemes: &[&str]) -> Option<(&'a str, Query<'a>)> {
    let text = text.trim();
    let body = match text.split_once(':') {
        Some((scheme, rest)) if schemes.iter().any(|s| scheme.eq_ignore_ascii_case(s)) => {
            rest.strip_prefix("//").unwrap_or(rest)
        }
        Some(_) => return None,
        None => text,
    };
    let (address, query) = body.split_once('?').unwrap_or((body, ""));
    let pairs = query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            (key.to_ascii_lowercase(), value)
        })
        .collect();
    Some((address, Query(pairs)))
}

fn tag_value(s: &str) -> Option<u32> {
    if s.is_empty() || s.len() > TAG_DIGITS || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

fn plain_amount(s: &str, places: usize, digits: usize) -> Option<String> {
    let (int, frac) = s.split_once('.').unwrap_or((s, ""));
    if !int.bytes().chain(frac.bytes()).all(|b| b.is_ascii_digit()) {
        return None;
    }
    let int = int.trim_start_matches('0');
    let frac = frac.trim_end_matches('0');
    if (int.is_empty() && frac.is_empty()) || frac.len() > places || int.len() + frac.len() > digits {
        return None;
    }
    let int = if int.is_empty() { "0" } else { int };
    Some(if frac.is_empty() { int.to_string() } else { format!("{int}.{frac}") })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ADDR: &str = "rpcbQcdbjQrPvAVDzymF95vqht9M4wAzkc";
    const TAGGED: &str = "X7Yoo9fNvD5LCE49ScoNk3EKPL9wVee2mbUcQgFj8AXmcVt"; // ADDR with tag 104829
    const RLUSD_HEX: &str = "524C555344000000000000000000000000000000";
    const RLUSD_ISSUER: &str = "rMxCKbEDwqr76QuheSUMdEGf4B9xJ8m5De";
    const BTC: &str = "bc1qcr8te4kr609gcawutmrza0j4xv80jy8z306fyu";

    fn xrp(amount: &str) -> Option<Request<'_>> {
        Some(Request { amount, token: None })
    }

    fn rlusd(amount: &str) -> Option<Request<'_>> {
        Some(Request { amount, token: Some("RLUSD") })
    }

    fn code(to: &str, tag: Option<u32>, amount: Option<&str>, token: Option<&'static str>) -> XrpCode {
        XrpCode { to: to.to_string(), tag, amount: amount.map(str::to_string), token }
    }

    #[test]
    fn the_tagged_vector_is_the_address_and_its_tag() {
        let resolved = xaddress::resolve(TAGGED).unwrap();
        assert_eq!((resolved.classic.as_str(), resolved.tag), (ADDR, Some(104829)));
    }

    #[test]
    fn builds_the_vectors() {
        assert_eq!(xrp_payload(ADDR, None, None).as_deref(), Some(ADDR));
        assert_eq!(xrp_payload(ADDR, Some(104829), None).as_deref(), Some(TAGGED));
        assert_eq!(xrp_payload(ADDR, None, xrp("25")).unwrap(), format!("xrpl://{ADDR}?amount=25"));
        assert_eq!(xrp_payload(ADDR, None, xrp("7.70")).unwrap(), format!("xrpl://{ADDR}?amount=7.7"));
        assert_eq!(xrp_payload(ADDR, Some(104829), xrp("25")).unwrap(), format!("xrpl://{TAGGED}?amount=25"));
        assert_eq!(
            xrp_payload(ADDR, None, rlusd("25")).unwrap(),
            format!("xrpl://{ADDR}?amount=25&currency={RLUSD_HEX}&issuer={RLUSD_ISSUER}")
        );
        assert_eq!(
            xrp_payload(ADDR, Some(104829), rlusd("25")).unwrap(),
            format!("xrpl://{TAGGED}?amount=25&currency={RLUSD_HEX}&issuer={RLUSD_ISSUER}")
        );
    }

    #[test]
    fn builds_nothing_it_could_not_read_back() {
        assert_eq!(xrp_payload(&ADDR.to_lowercase(), None, None), None);
        assert_eq!(xrp_payload(ADDR, None, xrp("0")), None);
        assert_eq!(xrp_payload(ADDR, None, xrp("1.1234567")), None);
        assert_eq!(xrp_payload(ADDR, None, Some(Request { amount: "5", token: Some("EURØP") })), None);
    }

    #[test]
    fn normalises_typed_amounts() {
        let typed = [("25", "25"), ("25.", "25"), ("7.70", "7.7"), (".5", "0.5"), ("0.50", "0.5"), ("0025", "25"), ("0.000001", "0.000001")];
        for (raw, plain) in typed {
            assert_eq!(plain_amount(raw, XRP_PLACES, XRP_DIGITS).as_deref(), Some(plain), "{raw}");
        }
        let refused = ["", ".", "0", "0.000", "1.1234567", "1e5", "1,000", "-1", "+1", "1.2.3", " 1", "1234567890123456"];
        for raw in refused {
            assert_eq!(plain_amount(raw, XRP_PLACES, XRP_DIGITS), None, "{raw}");
        }
    }

    #[test]
    fn reads_the_vectors() {
        assert_eq!(read_xrp(ADDR), Ok(code(ADDR, None, None, None)));
        assert_eq!(read_xrp(TAGGED), Ok(code(TAGGED, None, None, None)));
        assert_eq!(read_xrp(&format!("{ADDR}?dt=104829")), Ok(code(ADDR, Some(104829), None, None)));
        assert_eq!(read_xrp(&format!("XRPL://{ADDR}?amount=25")), Ok(code(ADDR, None, Some("25"), None)));
        assert_eq!(read_xrp(&format!("ripple:{ADDR}?amount=7.7&dt=9")), Ok(code(ADDR, Some(9), Some("7.7"), None)));
        assert_eq!(read_xrp(&format!("xrpl:{ADDR}?dt=7&tag=7")), Ok(code(ADDR, Some(7), None, None)));
        assert_eq!(
            read_xrp(&format!("xrpl://{ADDR}?amount=25&currency={RLUSD_HEX}&issuer={RLUSD_ISSUER}")),
            Ok(code(ADDR, None, Some("25"), Some("RLUSD")))
        );
        assert_eq!(
            read_xrp(&format!(
                "xrpl://{ADDR}?amount=2&network=XRPL&dt=123&issuer={RLUSD_ISSUER}&currency={}",
                RLUSD_HEX.to_lowercase()
            )),
            Ok(code(ADDR, Some(123), Some("2"), Some("RLUSD")))
        );
    }

    #[test]
    fn reads_back_what_it_builds() {
        let cases = [
            (None, None),
            (Some(104829), None),
            (None, xrp("25")),
            (Some(104829), xrp("7.70")),
            (None, rlusd("25")),
            (Some(0), rlusd("0.5")),
        ];
        for (tag, request) in cases {
            let built = xrp_payload(ADDR, tag, request).unwrap();
            let read = read_xrp(&built).unwrap();
            let resolved = xaddress::resolve(&read.to).unwrap();
            assert_eq!(resolved.classic, ADDR, "{built}");
            assert_eq!(resolved.tag.or(read.tag), tag, "{built}");
            let amount = request.map(|r| plain_amount(r.amount, XRP_PLACES, XRP_DIGITS).unwrap());
            assert_eq!(read.amount, amount, "{built}");
            assert_eq!(read.token, request.and_then(|r| r.token), "{built}");
        }
    }

    #[test]
    fn refuses_what_it_cannot_pay_as_written() {
        let refused = [
            format!("https://xaman.app/detect/request:{ADDR}?amount=2"),
            format!("xrpl://{ADDR}?amount=2&network=XAHAU"),
            format!("xrpl://{ADDR}?amount=25&currency={RLUSD_HEX}"),
            format!("xrpl://{ADDR}?amount=25&issuer={RLUSD_ISSUER}"),
            format!("{ADDR}?dt=4294967296"),
            format!("{ADDR}?dt=-1"),
            format!("{ADDR}?dt=1&tag=2"),
            format!("{ADDR}:104829"),
            format!("{TAGGED}?dt=104829"),
            format!("xrpl://{ADDR}?amount=1.1234567"),
            format!("xrpl://{ADDR}?amount=0"),
            format!("xrpl://{ADDR}?amount=1&amount=1000"),
            format!("bitcoin:{BTC}"),
            ADDR.to_lowercase(),
            String::new(),
        ];
        for text in refused {
            assert_eq!(read_xrp(&text), Err(CodeError::Refused), "{text}");
        }
    }

    #[test]
    fn names_a_token_the_list_does_not_hold() {
        let other_issuer = format!("xrpl://{ADDR}?amount=5&currency={RLUSD_HEX}&issuer={ADDR}");
        let three_letters = format!("xrpl://{ADDR}?amount=5&currency=USD&issuer={RLUSD_ISSUER}");
        for text in [other_issuer, three_letters] {
            assert_eq!(read_xrp(&text), Err(CodeError::TokenNotSupported), "{text}");
        }
    }

    #[test]
    fn builds_and_reads_bip21() {
        assert_eq!(btc_payload(BTC, None).as_deref(), Some(BTC));
        assert_eq!(btc_payload(BTC, Some("0.001250")).unwrap(), format!("bitcoin:{BTC}?amount=0.00125"));
        assert_eq!(btc_payload(BTC, Some("0.000000001")), None);
        assert_eq!(read_btc(BTC), Ok(BtcCode { to: BTC.to_string(), amount: None }));
        assert_eq!(
            read_btc(&format!("BITCOIN:{BTC}?amount=0.00125&label=Shop")),
            Ok(BtcCode { to: BTC.to_string(), amount: Some("0.00125".to_string()) })
        );
        let refused = [
            format!("bitcoin:{BTC}?req-pop=1"),
            format!("bitcoin:{BTC}?amount=0.000000001"),
            format!("bitcoin:{BTC}?amount=1,5"),
            format!("xrpl://{BTC}"),
            ADDR.to_string(),
        ];
        for text in refused {
            assert_eq!(read_btc(&text), Err(CodeError::Refused), "{text}");
        }
    }
}
