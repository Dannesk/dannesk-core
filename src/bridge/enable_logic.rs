use crate::channel::{CHANNEL, ActivityLogState, WSCommand};
use tokio::sync::mpsc::Sender;
use crate::secure::SecureString;


pub const ENABLE_LIMIT: &str = "1000000";

pub const DISABLE_LIMIT: &str = "0";

pub struct TrustlineEnableLogic;

impl TrustlineEnableLogic {
    #[allow(clippy::too_many_arguments)]
    pub async fn process(
        mode: String,
        passphrase: SecureString,
        mnemonic: SecureString,
        bip39_pass: SecureString,
        wallet_address: String,
        asset: String,
        limit: &'static str,
        title: &'static str,
        ws_tx: Sender<WSCommand>,
    ) {
        let Ok(mut log) = ActivityLogState::begin(
            title,
            &[
                ("init",      "Initializing"),
                ("auth",      "Authenticating"),
                ("build",     "Constructing transaction"),
                ("broadcast", "Broadcasting to network"),
                ("confirm",   "Awaiting response"),

            ],
            "relay:xrp",
        ) else {
            return;
        };

        // Move the locked secrets into the command — no clone, no unlocked copy.
        let bip39_opt = if bip39_pass.is_empty() {
            None
        } else {
            Some(bip39_pass)
        };
        let (passphrase, seed) = match mode.as_str() {
            "passphrase" => (
                if passphrase.is_empty() { None } else { Some(passphrase) },
                None,
            ),
            "seed" => (
                None,
                if mnemonic.as_str().trim().is_empty() { None } else { Some(mnemonic) },
            ),
                        _ => (None, None),
        };

        let cmd = WSCommand {
            command: "submit_transaction".to_string(),
            wallet: Some(wallet_address),
            recipient: None,
            amount: None,
            passphrase,
            seed,
            trustline_limit: Some(limit.to_string()),
            fee: None,
            tx_type: Some("trustset".to_string()),
            taker_pays: None,
            taker_gets: None,
            flags: None,
            wallet_type: Some(asset),
            bip39: bip39_opt,
            offer_sequence: None,
            destination_tag: None,
            scan: None,
            replaces: None,
            history_kind: None,
            history_offset: None,
            pending: None,
            xpub: None,
            script_type: None,
        };

        if let Err(e) = ws_tx.try_send(cmd) {
            log.fail("init", format!("Dispatch failed: {}", e));
            let _ = CHANNEL.activity_tx.send(Some(log));
        }
    }
}
