//! Backend-rendered strings.

use std::collections::HashMap;
use std::sync::RwLock;

static STRINGS: RwLock<Option<HashMap<String, String>>> = RwLock::new(None);

pub fn set(strings: HashMap<String, String>) {
    *STRINGS.write().unwrap() = Some(strings);
}

/// The keys under `prefix` of a locale file (`crates/gui/src-ui/locales/*.yaml`: two-space nesting,
/// double-quoted values), flattened to `a.b.c`, for a client without a screen to send them.
pub fn from_locale(text: &str, prefix: &str) -> HashMap<String, String> {
    let mut path: Vec<String> = Vec::new();
    let mut out = HashMap::new();
    for line in text.lines() {
        let body = line.trim_start();
        if body.is_empty() || body.starts_with('#') {
            continue;
        }
        let depth = (line.len() - body.len()) / 2;
        let Some((key, value)) = body.split_once(':') else {
            continue;
        };
        path.truncate(depth);
        let value = value.trim();
        if value.is_empty() {
            path.push(key.to_string());
            continue;
        }
        let full = path
            .iter()
            .chain([&key.to_string()])
            .cloned()
            .collect::<Vec<_>>()
            .join(".");
        if full.starts_with(prefix) {
            let v = value
                .strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .unwrap_or(value);
            out.insert(full, v.replace("\\\"", "\"").replace("\\\\", "\\"));
        }
    }
    out
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

#[cfg(test)]
mod tests {
    use super::from_locale;

    #[test]
    fn a_locale_file_flattens_to_dotted_keys() {
        let text = "a:\n  x: \"no\"\nbackend:\n  notify:\n    title: \"GGS: \\\"hi\\\"\"\n  filter:\n    log: \"Log\"\n";
        let m = from_locale(text, "backend.");
        assert_eq!(m.len(), 2);
        assert_eq!(m["backend.notify.title"], "GGS: \"hi\"");
        assert_eq!(m["backend.filter.log"], "Log");
    }
}
