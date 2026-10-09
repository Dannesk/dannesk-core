pub mod json_storage;
pub mod order_record;
pub mod xrp_create_logic;
pub mod xrp_import_logic;
pub mod xrp_send_logic;
pub mod cancel_logic;
pub mod trade_logic;
pub mod btc_send_logic;
pub mod btc_bump_logic;
pub mod btc_create_logic;
pub mod btc_import_logic;
pub mod btc_receive_rotation;
pub mod btc_wallet_operations;
pub mod xrp_wallet_operations;
pub mod enable_logic;

/// The path every XRP key is derived on, for the recovery-phrase eyebrow.
/// The frozen derivers spell it out themselves; the test below pins their
/// literals to this copy.
pub const XRP_PATH: &str = "m/44'/144'/0'/0/0";

#[cfg(test)]
mod tests {
    use super::XRP_PATH;
    use crate::btc_script_type::BtcScriptType;

    /// The eyebrow must state the path the derivers actually walk. The
    /// literals live in the (frozen) bridge files; these read them back
    /// rather than trusting anyone to keep the strings in step by hand.
    #[test]
    fn xrp_path_is_the_derivers() {
        for (file, deriver) in [
            ("xrp_create_logic.rs", include_str!("xrp_create_logic.rs")),
            ("xrp_import_logic.rs", include_str!("xrp_import_logic.rs")),
        ] {
            assert!(deriver.contains(&format!("\"{XRP_PATH}\"")), "XRP_PATH is not the path {file} derives");
        }
    }

    #[test]
    fn native_path_is_the_derivers() {
        let path = BtcScriptType::NativeSegwit.path();
        for (file, deriver) in [
            ("btc_create_logic.rs", include_str!("btc_create_logic.rs")),
            ("btc_import_logic.rs", include_str!("btc_import_logic.rs")),
        ] {
            assert!(deriver.contains(&format!("\"{path}\"")), "the native path is not the path {file} derives");
        }
    }
}

