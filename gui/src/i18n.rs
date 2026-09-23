//! Backend-rendered strings.

use std::collections::HashMap;
use std::sync::RwLock;

static STRINGS: RwLock<Option<HashMap<String, String>>> = RwLock::new(None);

pub fn set(strings: HashMap<String, String>) {
    *STRINGS.write().unwrap() = Some(strings);
}

pub fn t(key: &str) -> String {
    STRINGS
        .read()
        .unwrap()
        .as_ref()
        .and_then(|m| m.get(key))
        .cloned()
        .unwrap_or_else(|| key.to_string())
}

pub fn tf(key: &str, params: &[(&str, &str)]) -> String {
    let mut s = t(key);
    for (name, value) in params {
        s = s.replace(&format!("{{{name}}}"), value);
    }
    s
}
