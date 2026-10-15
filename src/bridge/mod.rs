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


pub const XRP_PATH: &str = "m/44'/144'/0'/0/0";

#[cfg(test)]
mod tests {
    use super::XRP_PATH;
    use crate::btc_script_type::BtcScriptType;

  
    #[test]
    fn xrp_path_is_the_derivers() {
        for (file, deriver) in [
            ("xrp_create_logic.rs", include_str!("xrp_create_logic.rs")),
            ("xrp_import_logic.rs", include_str!("xrp_import_logic.rs")),
            ("xrp_wallet_operations.rs", include_str!("xrp_wallet_operations.rs")),
            ("ws/commands/wallet_auth.rs", include_str!("../ws/commands/wallet_auth.rs")),
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
            ("btc_wallet_operations.rs", include_str!("btc_wallet_operations.rs")),
            ("ws/commands/bitcoin_auth.rs", include_str!("../ws/commands/bitcoin_auth.rs")),
        ] {
            assert!(deriver.contains(&format!("\"{path}\"")), "the native path is not the path {file} derives");
        }
    }
}

