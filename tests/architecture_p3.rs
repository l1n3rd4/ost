//! Automated architecture check — Property 3: Ports domain-typed.
//!
//! Validates: Requirements 3.1, 3.2, 4.1, 4.3, 4.7, 2.2
//!
//! Formal property (design.md):
//!   ∀ port sig s : types(s) ⊆ DomainModels ∪ {DomainResult<T>}.
//!   ∀ s : {reqwest::Response, serde_json::Value, toml::Value} ∩ types(s) = ∅.
//!
//! This test source-scans every `.rs` file under `src/ports/` and fails if any
//! port *code* (trait/fn signature or any other non-comment line) references a
//! forbidden infrastructure type. It asserts that ports state the app's needs
//! purely in domain terms, with no infra type leaking across the boundary.
//!
//! ## Why comment lines are stripped
//!
//! The port modules *intentionally* name the forbidden tokens inside their doc
//! comments (e.g. api.rs documents that "`reqwest::Response`, `serde_json::Value`,
//! `toml::Value`, ... never appears in a public signature"). Those mentions are
//! documentation, not signatures, so scanning raw file text would false-positive
//! on correct code. To stay robust, every line whose trimmed content begins with
//! `//` (covering `//!` inner-doc, `///` outer-doc, and plain `//` comments) is
//! removed before the forbidden-token scan. What remains is the actual Rust code
//! — `use` items, trait definitions, and fn signatures — which is exactly the
//! surface Property 3 constrains.

use std::fs;
use std::path::{Path, PathBuf};

/// Infrastructure tokens that must never appear in port code.
///
/// Anchored on the design's explicit trio (`reqwest::Response`,
/// `serde_json::Value`, `toml::Value`) and broadened to catch any leak of the
/// underlying infra crates in a port signature.
const FORBIDDEN_TOKENS: &[&str] = &[
    "reqwest::",
    "reqwest",
    "serde_json::Value",
    "serde_json",
    "toml::Value",
    "toml::",
    "tokio_tungstenite",
    "tokio-tungstenite",
];

/// Return true if `trimmed` (an already-trimmed line) is a comment line.
///
/// Covers inner-doc (`//!`), outer-doc (`///`), and plain (`//`) comments — all
/// of which start with `//` once leading whitespace is removed.
fn is_comment_line(trimmed: &str) -> bool {
    trimmed.starts_with("//")
}

/// Collect every `.rs` file under `dir`, recursively.
fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("failed to read ports directory {}: {e}", dir.display()));
    for entry in entries {
        let entry = entry.expect("failed to read directory entry");
        let path = entry.path();
        if path.is_dir() {
            collect_rs_files(&path, out);
        } else if path.extension().map(|e| e == "rs").unwrap_or(false) {
            out.push(path);
        }
    }
}

#[test]
fn ports_reference_only_domain_types() {
    let ports_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join("ports");
    assert!(
        ports_dir.is_dir(),
        "expected ports directory at {}",
        ports_dir.display()
    );

    let mut files = Vec::new();
    collect_rs_files(&ports_dir, &mut files);
    files.sort();
    assert!(
        !files.is_empty(),
        "found no .rs files under {}",
        ports_dir.display()
    );

    let mut violations: Vec<String> = Vec::new();

    for file in &files {
        let contents = fs::read_to_string(file)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", file.display()));

        for (idx, raw_line) in contents.lines().enumerate() {
            let trimmed = raw_line.trim_start();

            // Skip comment lines: the design intentionally NAMES the forbidden
            // tokens in doc comments, so only non-comment code is scanned.
            if is_comment_line(trimmed) {
                continue;
            }

            for token in FORBIDDEN_TOKENS {
                if raw_line.contains(token) {
                    violations.push(format!(
                        "{}:{}: forbidden infra type `{}` in port code: `{}`",
                        file.display(),
                        idx + 1,
                        token,
                        raw_line.trim()
                    ));
                }
            }
        }
    }

    assert!(
        violations.is_empty(),
        "Property 3 violated — port signatures must reference only domain types \
         (no infra leaks). Offending lines:\n{}",
        violations.join("\n")
    );
}
