//! Backend-rendered strings.
//!
//! Almost every user-visible string is translated in the frontend from
//! `src-ui/locales/*.yaml`. Two kinds cannot be: OS notifications fired
//! from the GGS thread, and native file-dialog filter names. Those are
//! rendered here, so the frontend pushes its `backend.*` subset at
//! startup and on every language change (`set_backend_strings`).
//!
//! An unknown key renders as the key itself — the loudest possible sign
//! that the YAML and the code disagree.

use std::collections::HashMap;
use std::sync::RwLock;

static STRINGS: RwLock<Option<HashMap<String, String>>> = RwLock::new(None);

pub fn set(strings: HashMap<String, String>) {
    *STRINGS.write().unwrap() = Some(strings);
}

/// Look up `key`; unknown keys render as themselves.
pub fn t(key: &str) -> String {
    STRINGS
        .read()
        .unwrap()
        .as_ref()
        .and_then(|m| m.get(key))
        .cloned()
        .unwrap_or_else(|| key.to_string())
}

/// Look up `key` and fill `{name}` placeholders.
pub fn tf(key: &str, params: &[(&str, &str)]) -> String {
    let mut s = t(key);
    for (name, value) in params {
        s = s.replace(&format!("{{{name}}}"), value);
    }
    s
}
