//! Dannesk's core: everything under the user interface, shared by the desktop
//! app and Android. The one socket to the proxy and what its frames mean
//! (`ws`), the state bus an interface subscribes to (`channel`), wallet
//! creation, import, sending, signing and storage (`bridge`, `wallet`,
//! `encrypt`, `decrypt`, `secure`), the launch-PIN gate (`gate`) and the
//! chain-neutral helpers in `utils`.
//!
//! An app calls [`init`] once, before anything else, and names what the core
//! cannot know on its own: its version and the directory its files live in.
//! Everything platform-shaped stays in the app: the window or the Activity,
//! fonts, the clipboard, and where that directory is.

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
