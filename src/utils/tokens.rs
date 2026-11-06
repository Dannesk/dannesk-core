

pub struct TokenDef {

    pub code: &'static str,
    pub display: &'static str,
 
    pub ticker: &'static str,
    pub issuer_name: &'static str,
    pub issuer: &'static str,
    pub currency_hex: &'static str,

    pub rate_key: &'static str,
    pub balance_cmd: &'static str,
    pub balance_field: &'static str,
    pub has_field: &'static str,
    pub trustline_cmd: &'static str,
    pub trustline_limit_field: &'static str,
}

pub static TOKENS: &[TokenDef] = &[
    TokenDef {
        code: "RLUSD",
        display: "RLUSD",
        ticker: "RLUSD",
        issuer_name: "Ripple",
        issuer: "rMxCKbEDwqr76QuheSUMdEGf4B9xJ8m5De",
        currency_hex: "524C555344000000000000000000000000000000",
        rate_key: "RLUSD", 
        balance_cmd: "get_rlusd_balance",
        balance_field: "rlusd_balance",
        has_field: "has_rlusd",
        trustline_cmd: "get_trustline_limit",
        trustline_limit_field: "trustline_limit",
    },
    TokenDef {
        code: "EUROP",
        display: "EURØP",
        ticker: "EUROP",
        issuer_name: "Schuman Financial",
        issuer: "rMkEuRii9w9uBMQDnWV5AA43gvYZR9JxVK",
        currency_hex: "4555524F50000000000000000000000000000000",
        rate_key: "EUR",
        balance_cmd: "get_euro_balance",
        balance_field: "euro_balance",
        has_field: "has_euro",
        trustline_cmd: "get_trustline_euro_limit",
        trustline_limit_field: "trustline_euro_limit",
    },
    TokenDef {
        code: "XSGD",
        display: "XSGD",
        ticker: "XSGD",
        issuer_name: "StraitsX",
        issuer: "rK67JczCpaYXVtfw3qJVmqwpSfa1bYTptw",
        currency_hex: "5853474400000000000000000000000000000000",
        rate_key: "SGD",
        balance_cmd: "get_xsgd_balance",
        balance_field: "xsgd_balance",
        has_field: "has_xsgd",
        trustline_cmd: "get_trustline_sgd_limit",
        trustline_limit_field: "trustline_xsgd_limit",
    },
    TokenDef {
        code: "AUDD",
        display: "AUDD",
        ticker: "AUDD",
        issuer_name: "AUDC Pty Ltd",
        issuer: "rUN5Zxt3K1AnMRJgEWywDJT8QDMMeLH5ok",
        currency_hex: "4155444400000000000000000000000000000000",
        rate_key: "AUD",
        balance_cmd: "get_audd_balance",
        balance_field: "audd_balance",
        has_field: "has_audd",
        trustline_cmd: "get_trustline_audd_limit",
        trustline_limit_field: "trustline_audd_limit",
    },
    TokenDef {
        code: "USDC",
        display: "USDC",
        ticker: "USDC",
        issuer_name: "Circle",
        issuer: "rGm7WCVp9gb4jZHWTEtGUr4dd74z2XuWhE",
        currency_hex: "5553444300000000000000000000000000000000",
        rate_key: "USDC", 
        balance_cmd: "get_usdc_balance",
        balance_field: "usdc_balance",
        has_field: "has_usdc",
        trustline_cmd: "get_trustline_usdc_limit",
        trustline_limit_field: "trustline_usdc_limit",
    },
];

pub fn by_code(code: &str) -> Option<&'static TokenDef> {
    TOKENS.iter().find(|t| t.code == code)
}

pub fn by_balance_cmd(cmd: &str) -> Option<&'static TokenDef> {
    TOKENS.iter().find(|t| t.balance_cmd == cmd)
}

pub fn by_trustline_cmd(cmd: &str) -> Option<&'static TokenDef> {
    TOKENS.iter().find(|t| t.trustline_cmd == cmd)
}


pub fn rate_key(code: &str) -> &str {
    by_code(code).map(|t| t.rate_key).unwrap_or(code)
}

/// A ledger decimal in plain notation, the node's exponent form such as `1000000000000000e-30` written out in full.
pub fn plain_decimal(s: &str) -> Option<String> {
    let s = s.trim();
    let (mantissa, exponent) = match s.split_once(['e', 'E']) {
        Some((m, e)) => (m, e.parse::<i32>().ok()?),
        None => (s, 0),
    };
    let (int, frac) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let digits_only = |p: &str| p.bytes().all(|b| b.is_ascii_digit());
    if (int.is_empty() && frac.is_empty()) || !digits_only(int) || !digits_only(frac) {
        return None;
    }
    let digits = format!("{int}{frac}");
    let point = int.len() as i64 + exponent as i64;
    let (whole, part) = if point <= 0 {
        ("0".to_string(), format!("{}{digits}", "0".repeat(point.unsigned_abs() as usize)))
    } else if point as usize >= digits.len() {
        (format!("{digits}{}", "0".repeat(point as usize - digits.len())), String::new())
    } else {
        (digits[..point as usize].to_string(), digits[point as usize..].to_string())
    };
    let whole = whole.trim_start_matches('0');
    let part = part.trim_end_matches('0');
    let whole = if whole.is_empty() { "0" } else { whole };
    Some(if part.is_empty() { whole.to_string() } else { format!("{whole}.{part}") })
}

#[cfg(test)]
mod tests {
    use super::plain_decimal;

    #[test]
    fn a_ledger_decimal_is_written_out_in_full() {
        let cases = [
            ("1000000000000000e-30", "0.000000000000001"),
            ("9000000000000011e-15", "9.000000000000011"),
            ("1234e-2", "12.34"),
            ("5e3", "5000"),
            ("12.5", "12.5"),
            ("0.0040", "0.004"),
            ("0", "0"),
            ("007.10", "7.1"),
            ("9.000000000000011", "9.000000000000011"),
        ];
        for (ledger, plain) in cases {
            assert_eq!(plain_decimal(ledger).as_deref(), Some(plain), "{ledger}");
        }
        for bad in ["", "e5", "1.2.3", "-1", "abc", "1e"] {
            assert_eq!(plain_decimal(bad), None, "{bad:?}");
        }
    }
}
