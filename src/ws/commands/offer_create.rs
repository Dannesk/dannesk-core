// ws/commands/offer_create.rs
use crate::channel::WSCommand;
use dannesk_xrpl_codec::{Amount, TransactionType};
use crate::ws::commands::transaction_builder;
use crate::ws::commands::wallet_auth::Bip44Wallet;

const TF_IMMEDIATE_OR_CANCEL: u32 = 0x0002_0000;
const TF_FILL_OR_KILL: u32 = 0x0004_0000;
const TF_SELL: u32 = 0x0008_0000;


fn xrp_str_to_drops(xrp_str: &str) -> Result<u64, String> {
    if xrp_str.is_empty()
        || !xrp_str
            .chars()
            .all(|c| c.is_ascii_digit() || c == '.' || c == '-')
    {
        return Err(
            "Invalid XRP amount format: must be numeric with optional decimal.".to_string(),
        );
    }

    let negative = if xrp_str.starts_with('-') { -1 } else { 1 };
    let abs_str = xrp_str.trim_start_matches('-');

    let parts: Vec<&str> = abs_str.split('.').collect();
    if parts.len() > 2 {
        return Err("Invalid XRP amount: too many decimal points.".to_string());
    }

    let integer_part = parts[0].trim_start_matches('0'); 
    let integer_str = if integer_part.is_empty() {
        "0"
    } else {
        integer_part
    };

    let mut fractional_part = String::new();
    if parts.len() == 2 {
        fractional_part = parts[1].to_string();
    }

    
    while fractional_part.len() < 6 {
        fractional_part.push('0');
    }
    if fractional_part.len() > 6 {
        fractional_part.truncate(6);
    }

    let integer_drops: u128 = integer_str
        .parse()
        .map_err(|_| "Invalid integer part.".to_string())?;
    let fractional_drops: u128 = fractional_part
        .parse()
        .map_err(|_| "Invalid fractional part.".to_string())?;

    let total_drops = integer_drops
        .checked_mul(1_000_000)
        .and_then(|d| d.checked_add(fractional_drops))
        .and_then(|d| u64::try_from(d).ok())
        .ok_or("XRP amount is too large.")?;
    if negative < 0 && total_drops > 0 {
        return Err("Negative XRP amounts not supported.".to_string());
    }

    Ok(total_drops)
}

fn normalize_issued_value(amount_str: &str) -> Result<String, String> {
    let s = amount_str.trim();
    if s.is_empty() || !s.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return Err("Invalid amount format: must be a positive decimal number.".to_string());
    }

    let parts: Vec<&str> = s.split('.').collect();
    if parts.len() > 2 {
        return Err("Invalid amount: too many decimal points.".to_string());
    }
    let integer_part = parts[0];
    let fractional_part = if parts.len() == 2 { parts[1] } else { "" };

    let sig_digits = format!("{}{}", integer_part, fractional_part)
        .trim_start_matches('0')
        .trim_end_matches('0')
        .len();
    if sig_digits == 0 {
        return Err("Amount must be greater than zero.".to_string());
    }
    if sig_digits > 15 {
        return Err(
            "Amount has too many significant digits (max 15 for issued currencies).".to_string(),
        );
    }


    let int_norm = integer_part.trim_start_matches('0');
    let int_out = if int_norm.is_empty() { "0" } else { int_norm };
    let frac_norm = fractional_part.trim_end_matches('0');
    Ok(if frac_norm.is_empty() {
        int_out.to_string()
    } else {
        format!("{}.{}", int_out, frac_norm)
    })
}

fn get_asset_config(symbol: &str) -> Option<(&'static str, &'static str)> {
    crate::utils::tokens::by_code(symbol).map(|t| (t.currency_hex, t.issuer))
}


fn to_xrpl_amount(amount_str: &str, currency: &str) -> Result<Amount, String> {
    if currency == "XRP" {
        let drops = xrp_str_to_drops(amount_str)?;
        if drops == 0 {
            return Err("Amount must be greater than zero.".to_string());
        }
        Amount::xrp(drops)
    } else {
        let (hex, issuer) = get_asset_config(currency)
            .ok_or_else(|| format!("Unsupported currency: {}", currency))?;

        let value = normalize_issued_value(amount_str)?;

        Amount::issued(&value, hex, issuer)
    }
}

fn offer_flags(names: Option<&Vec<String>>) -> u32 {
    let mut flags = 0u32;
    if let Some(names) = names {
        for flag in names {
            match flag.as_str() {
                "tfFillOrKill" => flags |= TF_FILL_OR_KILL,
                "tfImmediateOrCancel" => flags |= TF_IMMEDIATE_OR_CANCEL,
              
                "tfSell" => flags |= TF_SELL,
                _ => (),
            }
        }
    }
    flags
}

// --- CORE LOGIC ---

pub async fn construct_blob(
    wallet_obj: &Bip44Wallet,
    cmd: &WSCommand,
    sequence: u32,
    fee: u64,
    last_ledger_sequence: u32,
) -> Result<String, String> {
    let taker_pays_raw = cmd.taker_pays.as_ref().ok_or("Missing taker_pays")?;
    let taker_gets_raw = cmd.taker_gets.as_ref().ok_or("Missing taker_gets")?;

    let taker_pays_amount = to_xrpl_amount(&taker_pays_raw.0, &taker_pays_raw.1)?;
    let taker_gets_amount = to_xrpl_amount(&taker_gets_raw.0, &taker_gets_raw.1)?;

   
    let fields = vec![
        dannesk_xrpl_codec::transaction_type(TransactionType::OfferCreate),
        dannesk_xrpl_codec::account(&wallet_obj.address)?,
        dannesk_xrpl_codec::fee(fee)?,
        dannesk_xrpl_codec::sequence(sequence),
        dannesk_xrpl_codec::last_ledger_sequence(last_ledger_sequence),
        dannesk_xrpl_codec::flags(offer_flags(cmd.flags.as_ref())),
        dannesk_xrpl_codec::taker_gets(taker_gets_amount),
        dannesk_xrpl_codec::taker_pays(taker_pays_amount),
    ];

    transaction_builder::sign(wallet_obj, fields)
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_fold_into_the_documented_bits() {
        let names = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(offer_flags(None), 0);
        assert_eq!(offer_flags(Some(&names(&[]))), 0);
        assert_eq!(offer_flags(Some(&names(&["tfFillOrKill"]))), 0x0004_0000);
        assert_eq!(offer_flags(Some(&names(&["tfImmediateOrCancel"]))), 0x0002_0000);
        assert_eq!(offer_flags(Some(&names(&["tfSell"]))), 0x0008_0000);
        assert_eq!(
            offer_flags(Some(&names(&["tfSell", "tfImmediateOrCancel", "bogus"]))),
            0x000A_0000
        );
    }

    #[test]
    fn xrp_for_token_offer_encodes() {
        let gets = to_xrpl_amount("25", "XRP").unwrap();
        let pays = to_xrpl_amount("10.50", "RLUSD").unwrap();
        let token = crate::utils::tokens::by_code("RLUSD").unwrap();
        assert_eq!(gets, Amount::xrp(25_000_000).unwrap());
        assert_eq!(pays, Amount::issued("10.5", token.currency_hex, token.issuer).unwrap());
        let fields = vec![
            dannesk_xrpl_codec::transaction_type(TransactionType::OfferCreate),
            dannesk_xrpl_codec::account("rLSn6Z3T8uCxbcd1oxwfGQN1Fdn5CyGujK").unwrap(),
            dannesk_xrpl_codec::fee(12).unwrap(),
            dannesk_xrpl_codec::sequence(1),
            dannesk_xrpl_codec::last_ledger_sequence(99),
            dannesk_xrpl_codec::flags(TF_SELL),
            dannesk_xrpl_codec::taker_gets(gets),
            dannesk_xrpl_codec::taker_pays(pays),
        ];
        let hex = hex::encode_upper(dannesk_xrpl_codec::encode(&fields).unwrap());
        assert!(hex.contains("2200080000"), "tfSell not encoded: {hex}");
    }
}
