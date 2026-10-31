

use dannesk_btc_codec::address::{Address, Kind};
use dannesk_btc_codec::secp256k1::PublicKey;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BtcScriptType {
  
    #[default]
    NativeSegwit,
    Taproot,
    NestedSegwit,
    Legacy,
}

impl BtcScriptType {

    pub const IMPORT: [Self; 4] = [Self::NativeSegwit, Self::Taproot, Self::NestedSegwit, Self::Legacy];


    pub const CREATE: [Self; 2] = [Self::NativeSegwit, Self::Taproot];

    pub fn purpose(self) -> u32 {
        match self {
            Self::NativeSegwit => 84,
            Self::Taproot => 86,
            Self::NestedSegwit => 49,
            Self::Legacy => 44,
        }
    }

  
    pub fn tag(self) -> &'static str {
        match self {
            Self::NativeSegwit => "native",
            Self::Taproot => "taproot",
            Self::NestedSegwit => "nested",
            Self::Legacy => "legacy",
        }
    }

    pub fn from_tag(tag: &str) -> Option<Self> {
        Self::IMPORT.into_iter().find(|t| t.tag() == tag)
    }


    pub fn prefix(self) -> &'static str {
        match self {
            Self::NativeSegwit => "bc1q",
            Self::Taproot => "bc1p",
            Self::NestedSegwit => "3\u{2026}",
            Self::Legacy => "1\u{2026}",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::NativeSegwit => "native segwit",
            Self::Taproot => "taproot",
            Self::NestedSegwit => "nested segwit",
            Self::Legacy => "legacy",
        }
    }

    pub fn bip(self) -> &'static str {
        match self {
            Self::NativeSegwit => "bip-84",
            Self::Taproot => "bip-86",
            Self::NestedSegwit => "bip-49",
            Self::Legacy => "bip-44",
        }
    }

    pub fn path(self) -> &'static str {
        match self {
            Self::NativeSegwit => "m/84'/0'/0'/0/0",
            Self::Taproot => "m/86'/0'/0'/0/0",
            Self::NestedSegwit => "m/49'/0'/0'/0/0",
            Self::Legacy => "m/44'/0'/0'/0/0",
        }
    }

    pub fn address(self, key: &PublicKey) -> Address {
        match self {
            Self::NativeSegwit => Address::p2wpkh(key),
            Self::Taproot => Address::p2tr(&key.x_only_public_key().0),
            Self::NestedSegwit => Address::p2sh_p2wpkh(key),
            Self::Legacy => Address::p2pkh(key),
        }
    }

  
    pub fn of_address(address: &str) -> Option<Self> {
        match Address::parse(address).ok()?.kind() {
            Kind::P2wpkh => Some(Self::NativeSegwit),
            Kind::P2tr => Some(Self::Taproot),
            Kind::P2sh => Some(Self::NestedSegwit),
            Kind::P2pkh => Some(Self::Legacy),
            Kind::P2wsh | Kind::OtherWitness => None,
        }
    }

    pub fn spk_len(self) -> usize {
        match self {
            Self::NativeSegwit => 22,
            Self::Taproot => 34,
            Self::NestedSegwit => 23,
            Self::Legacy => 25,
        }
    }

  
    pub fn script_sig_len(self) -> u64 {
        match self {
            Self::NativeSegwit | Self::Taproot => 0,
            Self::NestedSegwit => 23,
         
            Self::Legacy => 108,
        }
    }

  
    pub fn witness_len(self) -> u64 {
        match self {
            Self::NativeSegwit | Self::NestedSegwit => 109,
          
            Self::Taproot => 66,
            Self::Legacy => 0,
        }
    }
}


pub fn stored() -> BtcScriptType {
    crate::bridge::json_storage::read_json::<serde_json::Value>("btc.json")
        .ok()
        .and_then(|j| j.get("script_type").and_then(|v| v.as_str()).and_then(BtcScriptType::from_tag))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use dannesk_btc_codec::bip32::{DerivationPath, Xpriv};
    use dannesk_btc_codec::bip39::Mnemonic;

    const ABANDON: &str =
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    fn first_address(t: BtcScriptType) -> String {
        let seed = Mnemonic::parse(ABANDON).unwrap().to_seed("");
        let master = Xpriv::new_master(&seed).unwrap();
        let child = master.derive(&t.path().parse::<DerivationPath>().unwrap()).unwrap();
        t.address(&child.public_key()).to_string()
    }

  
    #[test]
    fn first_addresses_match_the_published_vectors() {
        assert_eq!(first_address(BtcScriptType::NativeSegwit), "bc1qcr8te4kr609gcawutmrza0j4xv80jy8z306fyu");
        assert_eq!(
            first_address(BtcScriptType::Taproot),
            "bc1p5cyxnuxmeuwuvkwfem96lqzszd02n6xdcjrs20cac6yqjjwudpxqkedrcr"
        );
        assert_eq!(first_address(BtcScriptType::NestedSegwit), "37VucYSaXLCAsxYyAPfbSi9eh4iEcbShgf");
        assert_eq!(first_address(BtcScriptType::Legacy), "1LqBGSKuX5yYUonjxT5qGfpUsXKYYWeabA");
    }

    
    #[test]
    fn a_derived_address_parses_back_to_its_type() {
        for t in BtcScriptType::IMPORT {
            assert_eq!(BtcScriptType::of_address(&first_address(t)), Some(t), "{t:?}");
        }
    }

    #[test]
    fn spk_lengths_are_the_real_scripts() {
        for t in BtcScriptType::IMPORT {
            let addr = Address::parse(&first_address(t)).unwrap();
            assert_eq!(addr.script_pubkey().len(), t.spk_len(), "{t:?}");
        }
    }

    #[test]
    fn tags_round_trip_and_native_is_the_default() {
        for t in BtcScriptType::IMPORT {
            assert_eq!(BtcScriptType::from_tag(t.tag()), Some(t));
        }
        assert_eq!(BtcScriptType::from_tag("p2wpkh"), None);
        assert_eq!(BtcScriptType::default(), BtcScriptType::NativeSegwit);
    }

    #[test]
    fn paths_carry_the_purpose() {
        for t in BtcScriptType::IMPORT {
            assert_eq!(t.path(), format!("m/{}'/0'/0'/0/0", t.purpose()));
        }
    }
}
