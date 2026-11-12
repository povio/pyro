//! Every `thread_local!` static of the crate is classified in
//! `pyro_webapi::thread_state::STATICS`: a browser thread serves one client
//! after another, and a static nobody looked at could carry one client's
//! state to the next.

use std::path::Path;

fn statics_in(dir: &Path, out: &mut Vec<String>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            statics_in(&path, out);
            continue;
        }
        if path.extension().is_none_or(|e| e != "rs") {
            continue;
        }
        let stem = path.file_stem().unwrap().to_str().unwrap();
        let module = if stem == "mod" {
            path.parent().unwrap().file_name().unwrap().to_str().unwrap().to_owned()
        } else {
            stem.to_owned()
        };
        let text = std::fs::read_to_string(&path).unwrap();
        let mut depth = 0i32;
        let mut inside = false;
        for line in text.lines() {
            let code = line.trim_start();
            if !inside && code.starts_with("thread_local!") {
                inside = true;
                depth = 0;
            }
            if !inside {
                continue;
            }
            // One static a line in the usual layout; the one-line form
            // (`thread_local! {static NAME:...}`) has it after the brace.
            for (at, _) in code.match_indices("static ") {
                let rest = &code[at + "static ".len()..];
                let name: String =
                    rest.chars().take_while(|c| c.is_ascii_uppercase() || *c == '_' || c.is_ascii_digit()).collect();
                if !name.is_empty() && rest[name.len()..].trim_start().starts_with(':') {
                    out.push(format!("{module}::{name}"));
                }
            }
            for c in code.chars() {
                match c {
                    '{' | '(' => depth += 1,
                    '}' | ')' => depth -= 1,
                    _ => {}
                }
            }
            if depth <= 0 && (code.contains('}') || code.contains(')')) {
                inside = false;
            }
        }
    }
}

#[test]
fn every_thread_local_is_classified() {
    let mut found = Vec::new();
    statics_in(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut found);
    found.sort();
    let mut listed: Vec<String> = pyro_webapi::thread_state::STATICS.iter().map(|s| (*s).to_owned()).collect();
    listed.sort();
    assert_eq!(
        found, listed,
        "a thread_local! static was added or removed: decide what a reused browser thread does with it \
         (src/thread_state.rs) and list it"
    );
}

#[test]
fn a_thread_nothing_used_is_clean() {
    assert_eq!(pyro_webapi::thread_state::reset_thread(), Ok(()));
}
