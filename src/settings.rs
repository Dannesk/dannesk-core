use crate::bridge::json_storage;
use crate::channel::CHANNEL;
use serde::Serialize;
use serde_json::Value;

const FILE: &str = "settings.json";

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Settings {
    pub theme: String,
    pub is_hidden: bool,
    pub base_currency: String,
    pub btc_sats: bool,
    pub gate_enabled: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: "Paper".to_string(),
            is_hidden: false,
            base_currency: "USD".to_string(),
            btc_sats: false,
            gate_enabled: true,
        }
    }
}

impl Settings {
    // Each key falls back to its default
    fn from_record(record: &Value) -> Self {
        let d = Self::default();
        Self {
            theme: record.get("theme").and_then(Value::as_str).map_or(d.theme, str::to_string),
            is_hidden: record.get("is_hidden").and_then(Value::as_bool).unwrap_or(d.is_hidden),
            base_currency: record.get("base_currency").and_then(Value::as_str).map_or(d.base_currency, str::to_string),
            btc_sats: record.get("btc_sats").and_then(Value::as_bool).unwrap_or(d.btc_sats),
            gate_enabled: record.get("gate_enabled").and_then(Value::as_bool).unwrap_or(d.gate_enabled),
        }
    }
}

pub fn load() {
    match json_storage::read_json::<Value>(FILE).ok().filter(Value::is_object) {
        Some(record) => {
            let _ = CHANNEL.settings_tx.send(Settings::from_record(&record));
        }
        None => reset(),
    }
}

pub fn reset() {
    let defaults = Settings::default();
    let _ = json_storage::write_json(FILE, &defaults);
    let _ = CHANNEL.settings_tx.send(defaults);
}

// Merges the settings into the record
fn set(change: impl FnOnce(&mut Settings)) {
    let mut next = CHANNEL.settings_rx.borrow().clone();
    change(&mut next);
    if let Ok(Value::Object(fields)) = serde_json::to_value(&next) {
        let _ = json_storage::update_json::<Value>(FILE, |record| {
            if let Some(obj) = record.as_object_mut() {
                obj.extend(fields);
            }
        });
    }
    let _ = CHANNEL.settings_tx.send(next);
}

pub fn set_theme(name: &str) {
    set(|s| s.theme = name.to_string());
}

pub fn set_hidden(on: bool) {
    set(|s| s.is_hidden = on);
}

pub fn set_base_currency(code: &str) {
    set(|s| s.base_currency = code.to_string());
}

pub fn set_btc_sats(on: bool) {
    set(|s| s.btc_sats = on);
}

pub fn set_gate_enabled(on: bool) {
    set(|s| s.gate_enabled = on);
}

#[cfg(test)]
mod tests {
    use super::Settings;
    use serde_json::json;

    #[test]
    fn a_missing_or_wrong_key_takes_its_default_alone() {
        let s = Settings::from_record(&json!({ "theme": "Graphite", "is_hidden": "yes", "gate_enabled": false }));
        assert_eq!(s.theme, "Graphite");
        assert!(!s.is_hidden);
        assert_eq!(s.base_currency, "USD");
        assert!(!s.btc_sats);
        assert!(!s.gate_enabled);
    }

    #[test]
    fn the_defaults_hold_every_key() {
        let written = serde_json::to_value(Settings::default()).unwrap();
        assert_eq!(
            written,
            json!({ "theme": "Paper", "is_hidden": false, "base_currency": "USD", "btc_sats": false, "gate_enabled": true })
        );
    }
}
