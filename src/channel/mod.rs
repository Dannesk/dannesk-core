pub mod btc;
pub mod global;
pub mod history;
pub mod xrp;
pub mod watchdog;

pub use btc::*;
pub use global::*;
pub use history::*;
pub use xrp::*;
pub use watchdog::*;

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;
use std::time::Duration;
use tokio::sync::watch;

pub type RateHistoryMap = HashMap<String, Vec<(u64, f32)>>;


#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ComponentHealth {
    pub up: bool,
    pub last_data_ms: u64,
}


#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Health {
    Up,
    Down,
    Unknown,
}

impl Health {
    pub fn is_up(self) -> bool {
        self == Health::Up
    }
}


#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RatesStatus {
    Live,
    Degraded,
    Checking,
    Offline,
}


#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Service {
    PriceFeeds,
    XrpNode,
    BitcoinNode,
    RelayServer,
}


#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceState {
    Up,
 
    Degraded,
    Down,
}


pub const PRICE_FEEDS: [&str; 3] = ["rates:binance", "rates:kraken", "rates:gemini"];
pub const ORDER_BOOK: &str = "bookd:xrpld";
pub const XRP_NODE: &str = "relay:xrp";

pub const BTC_NODE: &str = "btc:node";
pub const BTC_INDEX: &str = "btc:indexd";


#[derive(Debug, Clone, Default)]
pub struct ServiceHealth {
    pub components: HashMap<String, ComponentHealth>,

    pub stale_assets: HashSet<String>,
}


pub type TokenState = (f64, bool, Option<f64>);


#[derive(Debug, Clone, Default)]
pub struct OrderBook {
    pub ledger: u64,
    pub bids: Vec<(f64, f64)>,
    pub asks: Vec<(f64, f64)>,
    pub amm: Option<Amm>,
 
    pub require_auth: Option<bool>,
    pub global_freeze: Option<bool>,
  
    pub tick_size: Option<u32>,
}


#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct MarketSummary {
    pub ledger: u64,
    pub mid: f64,
    pub spread_pct: f64,
    pub bid1: f64,
    pub ask1: f64,
    pub bid5: f64,
    pub ask5: f64,
    pub amm_xrp: f64,
    pub amm1: f64,

    pub win_n: u32,
    pub win_mid: f64,
    pub win_spread_pct: f64,
    pub win_bid1: f64,
    pub win_ask1: f64,
    pub win_bid5: f64,
    pub win_ask5: f64,
    pub win_amm1: f64,
    pub holes: u32,
}


#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Amm {
    pub xrp: f64,
    pub token: f64,
    pub fee_pct: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KeyMode {
    #[default]
    Standard,
}

impl KeyMode {
  
    pub fn from_method(_method: &str) -> Self {
        KeyMode::Standard
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Loaded {

    pub wallets: bool,
   
    pub xrp: bool,
   
    pub btc: bool,
}


#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Launch {
    pub phase: Phase,
  
    pub over: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
   
    Connecting,

    Connected,
  
    Failed,
}


pub const FETCH_LIMIT: Duration = Duration::from_secs(10);

impl Launch {
    fn new() -> Self {
        Launch { phase: Phase::Connecting, over: false }
    }

    
    pub fn fetching(&self, link_up: bool, arrived: bool) -> bool {
        if arrived || self.over {
            return false;
        }
        match self.phase {
            Phase::Connecting => true,
            Phase::Connected => link_up,
            Phase::Failed => false,
        }
    }
}

pub static CHANNEL: LazyLock<Channel> = LazyLock::new(Channel::new);

pub struct Channel {
    pub rates_tx: watch::Sender<HashMap<String, f32>>,
    pub rates_rx: watch::Receiver<HashMap<String, f32>>,
    pub rate_history_tx: watch::Sender<RateHistoryMap>,
    pub rate_history_rx: watch::Receiver<RateHistoryMap>,
    pub rate_history_long_tx: watch::Sender<RateHistoryMap>,
    pub rate_history_long_rx: watch::Receiver<RateHistoryMap>,

    pub activity_tx: watch::Sender<Option<ActivityLogState>>,
    pub activity_rx: watch::Receiver<Option<ActivityLogState>>,
 
    pub rates_ws_status_tx: watch::Sender<bool>,
    pub rates_ws_status_rx: watch::Receiver<bool>,
    pub relay_ws_status_tx: watch::Sender<bool>,
    pub relay_ws_status_rx: watch::Receiver<bool>,
    pub btc_ws_status_tx: watch::Sender<bool>,
    pub btc_ws_status_rx: watch::Receiver<bool>,
    pub book_ws_status_tx: watch::Sender<bool>,
  
    pub proxy_ws_status_tx: watch::Sender<bool>,
    pub proxy_ws_status_rx: watch::Receiver<bool>,
  
    pub loaded_tx: watch::Sender<Loaded>,
    pub loaded_rx: watch::Receiver<Loaded>,
    pub launch_tx: watch::Sender<Launch>,
    pub launch_rx: watch::Receiver<Launch>,
    pub book_ws_status_rx: watch::Receiver<bool>,

    pub service_health_tx: watch::Sender<ServiceHealth>,
    pub service_health_rx: watch::Receiver<ServiceHealth>,

   
    pub tokens_tx: watch::Sender<HashMap<String, TokenState>>,
    pub tokens_rx: watch::Receiver<HashMap<String, TokenState>>,

  
    pub orderbook_tx: watch::Sender<HashMap<String, OrderBook>>,
    pub orderbook_rx: watch::Receiver<HashMap<String, OrderBook>>,
  
    pub markets_tx: watch::Sender<HashMap<String, MarketSummary>>,
    pub markets_rx: watch::Receiver<HashMap<String, MarketSummary>>,
    pub pending_trade_tx: watch::Sender<Option<PendingTrade>>,
    pub pending_trade_rx: watch::Receiver<Option<PendingTrade>>,

    pub wallet_balance_tx: watch::Sender<(f64, Option<String>, bool, KeyMode)>,
    pub wallet_balance_rx: watch::Receiver<(f64, Option<String>, bool, KeyMode)>,
    pub transactions_tx: watch::Sender<TransactionState>,
    pub transactions_rx: watch::Receiver<TransactionState>,
  
 
    pub btc_send_dispatched_tx: watch::Sender<BtcSendDispatches>,
    pub btc_send_dispatched_rx: watch::Receiver<BtcSendDispatches>,

    pub bitcoin_wallet_tx: watch::Sender<(f64, Option<String>, bool, KeyMode)>,
    pub bitcoin_wallet_rx: watch::Receiver<(f64, Option<String>, bool, KeyMode)>,
    pub btc_transactions_tx: watch::Sender<BtcTransactionState>,
    pub btc_transactions_rx: watch::Receiver<BtcTransactionState>,
  
    pub btc_utxos_tx: watch::Sender<(Option<String>, Vec<BtcUtxo>)>,
    pub btc_utxos_rx: watch::Receiver<(Option<String>, Vec<BtcUtxo>)>,
    pub btc_node_tx: watch::Sender<BtcNodeStats>,
    pub btc_node_rx: watch::Receiver<BtcNodeStats>,
    pub xrp_node_tx: watch::Sender<XrpNodeStats>,
    pub xrp_node_rx: watch::Receiver<XrpNodeStats>,
    pub xrp_account_tx: watch::Sender<XrpAccount>,
    pub xrp_account_rx: watch::Receiver<XrpAccount>,
}

impl Channel {
    pub fn new() -> Self {
        let (rates_tx, rates_rx) = watch::channel(HashMap::new());
        let (rate_history_tx, rate_history_rx) = watch::channel(HashMap::new());
        let (rate_history_long_tx, rate_history_long_rx) = watch::channel(HashMap::new());
        let (activity_tx, activity_rx) = watch::channel(None);
        let (btc_send_dispatched_tx, btc_send_dispatched_rx) = watch::channel(0);
        let (rates_ws_status_tx, rates_ws_status_rx) = watch::channel(false);
        let (relay_ws_status_tx, relay_ws_status_rx) = watch::channel(false);
        let (btc_ws_status_tx, btc_ws_status_rx) = watch::channel(false);
        let (book_ws_status_tx, book_ws_status_rx) = watch::channel(false);
        let (proxy_ws_status_tx, proxy_ws_status_rx) = watch::channel(false);
        let (loaded_tx, loaded_rx) = watch::channel(Loaded::default());
        let (launch_tx, launch_rx) = watch::channel(Launch::new());
        let (service_health_tx, service_health_rx) = watch::channel(ServiceHealth::default());
       
        let (tokens_tx, tokens_rx) = watch::channel(HashMap::new());

        let (orderbook_tx, orderbook_rx) = watch::channel(HashMap::new());
        let (markets_tx, markets_rx) = watch::channel(HashMap::new());
        let (pending_trade_tx, pending_trade_rx) = watch::channel(None);

        let (wallet_balance_tx, wallet_balance_rx) = watch::channel((0.0, None, false, KeyMode::Standard));

        let (transactions_tx, transactions_rx) = watch::channel(TransactionState::default());

        let (bitcoin_wallet_tx, bitcoin_wallet_rx) = watch::channel((0.0, None, false, KeyMode::Standard));

        let (btc_transactions_tx, btc_transactions_rx) = watch::channel(BtcTransactionState::default());

        let (btc_utxos_tx, btc_utxos_rx) = watch::channel((None, Vec::new()));

        let (btc_node_tx, btc_node_rx) = watch::channel(BtcNodeStats::default());
        let (xrp_node_tx, xrp_node_rx) = watch::channel(XrpNodeStats::default());
        let (xrp_account_tx, xrp_account_rx) = watch::channel(XrpAccount::default());


        Channel {

            rates_tx,
            rates_rx,
            rate_history_tx,
            rate_history_rx,
            rate_history_long_tx,
            rate_history_long_rx,
            btc_send_dispatched_tx,
            btc_send_dispatched_rx,
            activity_tx,
            activity_rx,
            rates_ws_status_tx,
            rates_ws_status_rx,
            relay_ws_status_tx,
            relay_ws_status_rx,
            btc_ws_status_tx,
            btc_ws_status_rx,
            book_ws_status_tx,
            proxy_ws_status_tx,
            proxy_ws_status_rx,
            loaded_tx,
            loaded_rx,
            launch_tx,
            launch_rx,
            book_ws_status_rx,
            service_health_tx,
            service_health_rx,


            tokens_tx,
            tokens_rx,

            orderbook_tx,
            orderbook_rx,
            markets_tx,
            markets_rx,
            pending_trade_tx,
            pending_trade_rx,

            wallet_balance_tx,
            wallet_balance_rx,
            transactions_tx,
            transactions_rx,
           

            bitcoin_wallet_tx,
            bitcoin_wallet_rx,
            btc_transactions_tx,
            btc_utxos_tx,
            btc_utxos_rx,
            btc_transactions_rx,
            btc_node_tx,
            btc_node_rx,
            xrp_node_tx,
            xrp_node_rx,
            xrp_account_tx,
            xrp_account_rx,

        }
    }


    pub fn book(&self, base: &str, quote: &str) -> Option<(String, OrderBook)> {
        let books = self.orderbook_rx.borrow();
        for key in [format!("{base}/{quote}"), format!("{quote}/{base}")] {
            if let Some(b) = books.get(&key) {
                return Some((key, b.clone()));
            }
        }
        None
    }

  
    pub fn token(&self, code: &str) -> TokenState {
        self.tokens_rx
            .borrow()
            .get(code)
            .copied()
            .unwrap_or((0.0, false, None))
    }


    pub fn set_token(&self, code: &str, state: TokenState) {
        self.tokens_tx
            .send_modify(|m| { m.insert(code.to_string(), state); });
    }

    pub fn set_token_balance(&self, code: &str, balance: f64) {
        self.tokens_tx.send_modify(|m| {
            m.entry(code.to_string()).or_insert((0.0, false, None)).0 = balance;
        });
    }

   
    pub fn clear_tokens(&self) {
        self.tokens_tx.send_modify(|m| m.clear());
        self.clear_xrp_account();
    }

    pub fn apply_xrp_account(&self, data: &serde_json::Value) {
        let seq = data.get("sequence").and_then(|v| v.as_u64()).and_then(|v| u32::try_from(v).ok());
        let oc = data.get("owner_count").and_then(|v| v.as_u64()).and_then(|v| u32::try_from(v).ok());
        if seq.is_none() && oc.is_none() {
            return;
        }
        self.xrp_account_tx.send_if_modified(|a| {
            let before = *a;
            if seq.is_some() { a.sequence = seq; }
            if oc.is_some() { a.owner_count = oc; }
            *a != before
        });
    }

    pub fn clear_xrp_account(&self) {
        let _ = self.xrp_account_tx.send(XrpAccount::default());
    }

    pub fn set_xrp_exists(&self, exists: bool) {
        self.tokens_tx.send_modify(|m| {
            m.entry("XRP".to_string()).or_insert((0.0, false, None)).1 = exists;
        });
    }

  
    pub fn health(&self, key: &str) -> Health {
        if !self.link_up(key) {
            return Health::Down;
        }
        match self.service_health_rx.borrow().components.get(key) {
            Some(c) if c.up => Health::Up,
            Some(_) => Health::Down,
            None => Health::Unknown,
        }
    }

    pub fn link_up(&self, key: &str) -> bool {
        match key.split(':').next() {
            Some("relay") => *self.relay_ws_status_rx.borrow(),
            Some("btc") => *self.btc_ws_status_rx.borrow(),
            Some("rates") => *self.rates_ws_status_rx.borrow(),
            Some("bookd") => *self.book_ws_status_rx.borrow(),
            _ => false,
        }
    }

  
    pub fn link_rx(&self, key: Option<&str>) -> watch::Receiver<bool> {
        match key.and_then(|k| k.split(':').next()) {
            Some("relay") => self.relay_ws_status_rx.clone(),
            Some("btc") => self.btc_ws_status_rx.clone(),
            Some("rates") => self.rates_ws_status_rx.clone(),
            Some("bookd") => self.book_ws_status_rx.clone(),
            _ => self.proxy_ws_status_rx.clone(),
        }
    }


    pub fn rates_status(&self) -> RatesStatus {
        if !*self.rates_ws_status_rx.borrow() {
            return RatesStatus::Offline;
        }
        let h = self.service_health_rx.borrow();
        if h.components.is_empty() {
            return RatesStatus::Checking;
        }
        if h.stale_assets.is_empty() {
            RatesStatus::Live
        } else {
            RatesStatus::Degraded
        }
    }

    pub fn service_state(&self, svc: Service) -> ServiceState {
        match svc {
            Service::PriceFeeds => {
             
                if !PRICE_FEEDS.iter().any(|k| self.health(k).is_up()) {
                    return ServiceState::Down;
                }
                match self.rates_status() {
                    RatesStatus::Live => ServiceState::Up,
                    RatesStatus::Degraded => ServiceState::Degraded,
                    RatesStatus::Checking | RatesStatus::Offline => ServiceState::Down,
                }
            }

    
            Service::XrpNode => {
                if !self.health(XRP_NODE).is_up() {
                    return ServiceState::Down;
                }
                let socket_up = *self.proxy_ws_status_rx.borrow();
                let book_link = *self.book_ws_status_rx.borrow();
                if socket_up && (!book_link || !self.health(ORDER_BOOK).is_up()) {
                    return ServiceState::Degraded;
                }
                ServiceState::Up
            }

          
            Service::BitcoinNode => {
                if self.health(BTC_NODE).is_up() && self.health(BTC_INDEX).is_up() {
                    ServiceState::Up
                } else {
                    ServiceState::Down
                }
            }

       
            Service::RelayServer => {
                if self.link_up(XRP_NODE) {
                    ServiceState::Up
                } else {
                    ServiceState::Down
                }
            }
        }
    }


    pub fn apply_health(
        &self,
        components: HashMap<String, ComponentHealth>,
        stale_assets: Option<HashSet<String>>,
    ) {
        self.service_health_tx.send_modify(|h| {
            h.components.extend(components);
            if let Some(stale) = stale_assets {
                h.stale_assets = stale;
            }
        });
    }

    
    pub fn mark_link_down(&self, prefix: &str) {
        self.service_health_tx.send_modify(|h| {
            for (key, c) in h.components.iter_mut() {
                if key.starts_with(prefix) {
                    c.up = false;
                }
            }
        });
    }

    
    pub fn xrp_exists(&self) -> bool {
        self.token("XRP").1
    }
}
