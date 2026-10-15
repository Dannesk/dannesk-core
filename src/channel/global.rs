use crate::btc_script_type::BtcScriptType;
use crate::channel::{Health, CHANNEL};
use crate::secure::SecureString;
use zeroize::{Zeroize, ZeroizeOnDrop};


#[derive(Debug, Clone, PartialEq)]
pub enum ActivityStepState {
    Pending,
    Active { since: std::time::Instant },
    Ok { ms: u64, at: std::time::Instant },
   
    Error { message: String },

    Stalled { message: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ActivityStep {
    pub id: &'static str,
    pub label: &'static str,
    pub state: ActivityStepState,
}

impl ActivityStep {
    pub fn pending(id: &'static str, label: &'static str) -> Self {
        Self { id, label, state: ActivityStepState::Pending }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ActivityLogState {
    pub title: &'static str,
    pub steps: Vec<ActivityStep>,

    pub note: Option<String>,

    pub health_key: Option<&'static str>,
}

impl ActivityLogState {
    pub fn new(title: &'static str, steps: &[(&'static str, &'static str)]) -> Self {
        Self {
            title,
            steps: steps.iter().map(|(id, label)| ActivityStep::pending(id, label)).collect(),
            note: None,
            health_key: None,
        }
    }


    pub fn begin(
        title: &'static str,
        steps: &[(&'static str, &'static str)],
        health_key: &'static str,
    ) -> Result<Self, String> {
        let mut log = Self::new(title, steps);
        log.health_key = Some(health_key);
        let Some((first, _)) = steps.first() else {
            return Ok(log);
        };
        log.start(first);
        let _ = CHANNEL.activity_tx.send(Some(log.clone()));

        if CHANNEL.health(health_key) == Health::Down {
            let msg = "No connection to the network — nothing was sent.".to_string();
            log.fail(first, msg.clone());
            let _ = CHANNEL.activity_tx.send(Some(log));
            return Err(msg);
        }
        Ok(log)
    }

    pub fn start(&mut self, id: &'static str) {
        if let Some(s) = self.steps.iter_mut().find(|s| s.id == id) {
            s.state = ActivityStepState::Active { since: std::time::Instant::now() };
        }
    }

    pub fn finish(&mut self, id: &'static str) {
        if let Some(s) = self.steps.iter_mut().find(|s| s.id == id) {
            let ms = match s.state {
                ActivityStepState::Active { since } => since.elapsed().as_millis() as u64,
                _ => 0,
            };
            s.state = ActivityStepState::Ok { ms, at: std::time::Instant::now() };
        }
    }

    pub fn fail(&mut self, id: &'static str, message: String) {
        if let Some(s) = self.steps.iter_mut().find(|s| s.id == id) {
            s.state = ActivityStepState::Error { message };
        }
    }

    pub fn all_ok(&self) -> bool {
        self.steps.iter().all(|s| matches!(s.state, ActivityStepState::Ok { .. }))
    }

    pub fn any_error(&self) -> bool {
        self.steps.iter().any(|s| {
            matches!(s.state, ActivityStepState::Error { .. } | ActivityStepState::Stalled { .. })
        })
    }

    pub fn is_terminal(&self) -> bool {
        self.all_ok() || self.any_error()
    }

 
    pub fn note(&mut self, message: String) {
        self.note = Some(message);
    }

    pub fn fail_active(&mut self, message: String) {
        for step in self.steps.iter_mut() {
            if matches!(step.state, ActivityStepState::Active { .. }) {
                step.state = ActivityStepState::Error { message };
                break;
            }
        }
    }

    
    pub fn stall_active(&mut self, message: String) {
        for step in self.steps.iter_mut() {
            if matches!(step.state, ActivityStepState::Active { .. }) {
                step.state = ActivityStepState::Stalled { message };
                break;
            }
        }
    }


    pub fn restate_failure(&mut self, message: String) {
        for step in self.steps.iter_mut() {
            if matches!(
                step.state,
                ActivityStepState::Active { .. } | ActivityStepState::Stalled { .. }
            ) {
                step.state = ActivityStepState::Error { message };
                return;
            }
        }
    }
}


#[derive(Zeroize, ZeroizeOnDrop)]
pub struct PendingWallet {
    pub address: String,
    pub encrypted_phrase: String,
    pub salt: String,
    pub iv: String,
    pub method: String,
 
    pub account_xpub: String,
   
    #[zeroize(skip)]
    pub script_type: BtcScriptType,
}

impl std::fmt::Debug for PendingWallet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingWallet")
            .field("address", &self.address)
            .field("method", &self.method)
            .field("script_type", &self.script_type)
            .finish_non_exhaustive()
    }
}


#[derive(Debug, Default)]
pub struct WSCommand {
    pub command: String,
    pub wallet: Option<String>,
    pub recipient: Option<String>,
    pub amount: Option<String>,
    pub passphrase: Option<SecureString>,
    pub trustline_limit: Option<String>,
    pub fee: Option<String>,
    pub tx_type: Option<String>,
    pub taker_pays: Option<(String, String)>,
    pub taker_gets: Option<(String, String)>,
    pub seed: Option<SecureString>,
    pub flags: Option<Vec<String>>,
    pub wallet_type: Option<String>,
    pub bip39: Option<SecureString>,
    pub offer_sequence: Option<u32>,

    pub destination_tag: Option<u32>,

    pub scan: Option<Vec<String>>,
   
    pub replaces: Option<String>,

    pub history_kind: Option<&'static str>,
    pub history_offset: Option<usize>,

    pub pending: Option<PendingWallet>,
  
    pub xpub: Option<String>,
    pub script_type: Option<&'static str>,
}
