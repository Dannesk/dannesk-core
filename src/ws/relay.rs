

use crate::channel::{CHANNEL, HistoryList, PendingWallet, WSCommand};
use crate::ws::commands::Command;
use crate::ws::config::{TAG_BTC, TAG_RELAY};
use serde::Serialize;
use serde_json::Value;
use tungstenite::Message;


#[derive(Serialize)]
struct WsMessage {
    payload: Value,
}

pub fn wrap(payload: &str) -> Option<(u8, String)> {
    let payload_json: Value = serde_json::from_str(payload).ok()?;
    let tag = match payload_json.get("command").and_then(|c| c.as_str()).and_then(Command::from_str) {
        Some(Command::ImportBitcoinWallet)
        | Some(Command::SubscribeBitcoinAddresses)
        | Some(Command::GetBitcoinBalance)
        | Some(Command::DeleteBitcoinWallet)
        | Some(Command::SubmitBitcoinTransaction)
        | Some(Command::GetBitcoinHistory) => TAG_BTC,
        _ => TAG_RELAY,
    };
    let wrapped = serde_json::to_string(&WsMessage { payload: payload_json }).ok()?;
    Some((tag, wrapped))
}


pub fn report_unsent(payload: &str) -> bool {
    let Ok(v) = serde_json::from_str::<Value>(payload) else { return false };
    match v.get("command").and_then(|c| c.as_str()) {
        Some("submit_transaction") => {
            crate::ws::commands::submit_transaction::report_unsent();
            true
        }
        Some("submit_bitcoin_transaction") => {
            crate::ws::commands::bitcoin_submit_transaction::report_unsent();
            true
        }
        Some("get_history") => {
            let list = match v.get("kind").and_then(|k| k.as_str()) {
                Some("orders") => HistoryList::XrpOrders,
                _ => HistoryList::XrpTransactions,
            };
            CHANNEL.transactions_tx.send_modify(|state| state.page_mut(list).fail_any());
            true
        }
        Some("get_bitcoin_history") => {
            CHANNEL.btc_transactions_tx.send_modify(|state| state.page.fail_any());
            true
        }
        _ => false,
    }
}


struct OwedDelete {
    command: String,
    wallet: String,
    /// Sent while its link was reported up; it stops being owed once a later report shows the link stayed up.
    written: bool,
}

#[derive(Default)]
pub struct RelayState {
    current_wallet: String,
 
    bitcoin_current_wallets: Vec<String>,

    btc_live_list: Vec<String>,
 
    pending_deletes: Vec<OwedDelete>,

    pending_xrp_import: Option<PendingWallet>,
    pending_xrp_create: Option<PendingWallet>,
    pending_btc: Option<PendingWallet>,
}

impl RelayState {
 
    pub fn is_news(&mut self, cmd: &WSCommand) -> bool {
        if cmd.command != "subscribe_bitcoin_addresses" {
            return true;
        }
        let mut list = cmd.scan.clone().unwrap_or_default();
        list.sort();
        if list == self.btc_live_list {
            return false;
        }
        self.btc_live_list = list;
        true
    }

 
    pub fn forget_live_list(&mut self) {
        self.btc_live_list.clear();
    }

  
    pub fn track(&mut self, cmd: &mut WSCommand) {
        if let Some(pending) = cmd.pending.take() {
            match cmd.command.as_str() {
                "import_wallet" => self.pending_xrp_import = Some(pending),
                "create_wallet" => self.pending_xrp_create = Some(pending),
                "import_bitcoin_wallet" => self.pending_btc = Some(pending),
               
                _ => {}
            }
        }
        if cmd.command == "get_cached_balance" {
            if let Some(w) = &cmd.wallet { self.current_wallet = w.clone(); }
        } else if cmd.command == "get_bitcoin_cached_balance" {
            if let Some(w) = &cmd.wallet
                && !self.bitcoin_current_wallets.contains(w)
            {
                self.bitcoin_current_wallets.push(w.clone());
            }
        } else if cmd.command == "delete_wallet" {
         
            self.current_wallet.clear();
            self.note_delete(cmd);
        } else if cmd.command == "delete_bitcoin_wallet" {
            self.bitcoin_current_wallets.clear();
            self.btc_live_list.clear();
            self.note_delete(cmd);
        }
    }

  
    fn note_delete(&mut self, cmd: &WSCommand) {
        let Some(wallet) = cmd.wallet.as_deref().filter(|w| !w.is_empty()) else { return };
        match self.pending_deletes.iter_mut().find(|d| d.command == cmd.command && d.wallet == wallet) {
            Some(owed) => owed.written = false,
            None => self.pending_deletes.push(OwedDelete {
                command: cmd.command.clone(),
                wallet: wallet.to_string(),
                written: false,
            }),
        }
    }


    fn clear_delete_for(&mut self, wallet: &str) {
        self.pending_deletes.retain(|d| d.wallet != wallet);
    }


    fn delete_payloads(&self, command: &str) -> Vec<String> {
        self.pending_deletes
            .iter()
            .filter(|d| d.command == command)
            .map(|d| serde_json::json!({ "command": d.command, "wallet": d.wallet }).to_string())
            .collect()
    }

 
    pub fn has_wallet(&self) -> bool {
        !self.current_wallet.is_empty() || !self.bitcoin_current_wallets.is_empty()
    }

    pub fn has_xrp_wallet(&self) -> bool {
        !self.current_wallet.is_empty()
    }

    pub fn deletes_written(&mut self, command: &str) {
        for owed in self.pending_deletes.iter_mut().filter(|d| d.command == command) {
            owed.written = true;
        }
    }

    pub fn deletes_unwritten(&mut self, command: &str) {
        for owed in self.pending_deletes.iter_mut().filter(|d| d.command == command) {
            owed.written = false;
        }
    }

    pub fn deletes_confirmed(&mut self, command: &str) {
        self.pending_deletes.retain(|d| d.command != command || !d.written);
    }

    pub fn written(&mut self, payload: &str) {
        let Ok(v) = serde_json::from_str::<Value>(payload) else { return };
        let (Some(command), Some(wallet)) = (v.get("command").and_then(|c| c.as_str()), v.get("wallet").and_then(|w| w.as_str())) else {
            return;
        };
        if let Some(owed) = self.pending_deletes.iter_mut().find(|d| d.command == command && d.wallet == wallet) {
            owed.written = true;
        }
    }

    
    pub fn spawn_command(&self, cmd: WSCommand) {
        if let Some(command) = Command::from_str(&cmd.command) {
            let wallet = self.current_wallet.clone();
         
            let btc_wallet = self.bitcoin_current_wallets.first().cloned().unwrap_or_default();
            tokio::spawn(async move {
                let _ = command.execute(wallet, btc_wallet, cmd).await;
            });
        }
    }


    pub fn resync_relay_payloads(&self) -> Vec<String> {
        let mut out = self.delete_payloads("delete_wallet");
        if !self.current_wallet.is_empty() {
            out.push(serde_json::json!({ "command": "get_cached_balance", "wallet": self.current_wallet }).to_string());
        }
        out
    }

    
    pub fn resync_btc_payloads(&self) -> Vec<String> {
        let mut out = self.delete_payloads("delete_bitcoin_wallet");
        let primary = self.bitcoin_current_wallets.first().cloned().unwrap_or_default();
        if let Some(payload) = crate::ws::commands::getbitcoincachedbalance::fetch_payload(&primary) {
            out.push(payload);
            return out;
        }
        out.extend(self.bitcoin_current_wallets.iter().map(|wallet| {
            serde_json::json!({ "command": "get_bitcoin_cached_balance", "wallet": wallet, "primary": primary })
                .to_string()
        }));
        out
    }

 
    pub async fn handle_frame(&mut self, text: String) {
        let Ok(data) = serde_json::from_str::<Value>(&text) else { return };
        match data.get("type").and_then(|t| t.as_str()) {
            Some("status") => { crate::ws::apply_status_frame(&data); return; }
            Some("node_stats") => { crate::ws::apply_node_stats_frame(&data); return; }
            Some("xrp_node_stats") => { crate::ws::apply_xrp_node_stats_frame(&data); return; }
            _ => {}
        }
        let cmd_str = data.get("command").and_then(|c| c.as_str());
        if let Some(command) = cmd_str.and_then(Command::from_str) {
            let btc_primary = self.bitcoin_current_wallets.first().cloned().unwrap_or_default();
        
            let pending = match command {
                Command::ImportWallet => self.pending_xrp_import.take(),
                Command::CreateWallet => self.pending_xrp_create.take(),
                Command::ImportBitcoinWallet => self.pending_btc.take(),
                _ => None,
            };
            let _ = command
                .process_response(Message::text(text), &self.current_wallet, &btc_primary, pending)
                .await;
            self.note_arrival(&data);
        }
    }

  
    fn note_arrival(&mut self, reply: &Value) {
        let Some(wallet) = reply.get("wallet").and_then(|w| w.as_str()) else { return };
        match reply.get("command").and_then(|c| c.as_str()) {
            Some("import_wallet") | Some("create_wallet") => {
                if CHANNEL.wallet_balance_rx.borrow().1.as_deref() == Some(wallet) {
                    self.current_wallet = wallet.to_string();
                    self.clear_delete_for(wallet);
                }
            }
            Some("import_bitcoin_wallet") => {
           
                if CHANNEL.bitcoin_wallet_rx.borrow().1.as_deref() == Some(wallet)
                    && self.bitcoin_current_wallets.first().map(String::as_str) != Some(wallet)
                {
                    self.bitcoin_current_wallets = vec![wallet.to_string()];
                }
                if CHANNEL.bitcoin_wallet_rx.borrow().1.as_deref() == Some(wallet) {
                    self.clear_delete_for(wallet);
                }
                self.btc_live_list.clear();
            }
            _ => {}
        }
    }
}
