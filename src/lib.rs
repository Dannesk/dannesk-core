//! Dannesk's core: communication layer

pub mod bridge;
pub mod btc_script_type;
pub mod channel;
pub mod decrypt;
pub mod encrypt;
pub mod gate;
pub mod secure;
pub mod utils;
pub mod wallet;
pub mod ws;

use std::path::PathBuf;
use std::sync::OnceLock;

#[derive(Debug, Clone)]
pub struct App {

    pub version: &'static str,

    pub data_dir: PathBuf,
}

static APP: OnceLock<App> = OnceLock::new();


pub fn init(app: App) -> Result<(), App> {
    APP.set(app)
}

pub fn app() -> Option<&'static App> {
    APP.get()
}
