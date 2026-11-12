fn main() {
    tauri_build::build();
    control_commands();
}

/// The commands the GUI's own screens call, with their arguments, read from the
/// `generate_handler!` list in `src/main.rs`, so the control socket offers exactly the same set.
fn control_commands() {
    println!("cargo:rerun-if-changed=src/main.rs");
    let src = std::fs::read_to_string("src/main.rs").expect("src/main.rs");
    let list = src
        .split_once("generate_handler![")
        .and_then(|(_, rest)| rest.split_once(']'))
        .map(|(l, _)| l)
        .expect("generate_handler! list");
    let mut out = String::from("pub const COMMANDS: &[(&str, &[(&str, &str)])] = &[\n");
    // A password never passes through the socket an AI drives.
    const SCREEN_ONLY: [&str; 1] = ["ggs_change_password"];
    for name in list
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty() && !SCREEN_ONLY.contains(s))
    {
        let params = params_of(&src, name);
        let ps: Vec<String> = params
            .iter()
            .map(|(n, t)| format!("({n:?}, {t:?})"))
            .collect();
        out.push_str(&format!("    ({name:?}, &[{}]),\n", ps.join(", ")));
    }
    out.push_str("];\n");
    let dest = std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("control_commands.rs");
    std::fs::write(dest, out).unwrap();
}

fn params_of(src: &str, name: &str) -> Vec<(String, String)> {
    let Some(at) = src.find(&format!("fn {name}(")) else {
        return Vec::new();
    };
    let body = &src[at + name.len() + 4..];
    let (mut depth, mut end) = (0i32, 0usize);
    for (i, c) in body.char_indices() {
        match c {
            '(' | '<' | '[' => depth += 1,
            ')' | '>' | ']' if depth > 0 => depth -= 1,
            ')' => {
                end = i;
                break;
            }
            _ => {}
        }
    }
    let mut parts = Vec::new();
    let (mut d, mut start) = (0i32, 0usize);
    let args = &body[..end];
    for (i, c) in args.char_indices() {
        match c {
            '(' | '<' | '[' => d += 1,
            ')' | '>' | ']' => d -= 1,
            ',' if d == 0 => {
                parts.push(&args[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&args[start..]);
    parts
        .into_iter()
        .filter_map(|p| p.trim().split_once(':'))
        .map(|(n, t)| {
            (
                n.trim().to_string(),
                t.split_whitespace().collect::<Vec<_>>().join(" "),
            )
        })
        .filter(|(_, t)| !(t.contains("State<") || t.contains("AppHandle") || t.contains("Window")))
        .collect()
}
