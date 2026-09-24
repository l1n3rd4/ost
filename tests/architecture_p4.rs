//! Automated architecture check — Property 4: No presentation in core.
//!
//! Validates: Requirements 5.1, 5.2
//!
//! Formal property (design.md):
//!   ∀ f ∈ (App ∪ Domain) : calls(f) ∩ {println!, print!, eprintln!, eprint!} = ∅.
//!
//! This test source-scans every `.rs` file under `src/app/` and `src/domain/`
//! and fails if any *code* line calls a stdout/stderr write macro. Business
//! logic must route all output through `PresenterPort`, never write directly,
//! so one logic path can serve CLI + TUI by swapping the presenter.
//!
//! ## Why comment lines are stripped
//!
//! The app/domain modules *intentionally* name the forbidden macros inside doc
//! comments (e.g. `app/mod.rs` documents that services "must never call
//! `println!`/`print!`/`eprintln!`/`eprint!`"). Those mentions are documentation,
//! not calls, so scanning raw file text would false-positive on correct code.
//! To stay robust, every line whose trimmed content begins with `//` (covering
//! `//!` inner-doc, `///` outer-doc, and plain `//` comments) is removed before
//! the forbidden-token scan. What remains is the actual Rust code — exactly the
//! surface Property 4 constrains.

use std::fs;
use std::path::{Path, PathBuf};

/// Presentation macros that must never appear in app/domain code.
///
/// Ordered longest-first so that reporting attributes each occurrence to its
/// most specific macro: `println!` and `eprintln!` are checked before their
/// `print!` / `eprint!` substrings, since `"print!"` is a substring of
/// `"println!"` and `"eprint!"` of `"eprintln!"`.
const FORBIDDEN_MACROS: &[&str] = &["println!", "eprintln!", "eprint!", "print!"];

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
        .unwrap_or_else(|e| panic!("failed to read directory {}: {e}", dir.display()));
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

/// Return the most-specific forbidden macro present on `line`, if any.
///
/// Checks longest names first so `println!` is reported as `println!` rather
/// than its `print!` substring.
fn first_forbidden_macro(line: &str) -> Option<&'static str> {
    FORBIDDEN_MACROS
        .iter()
        .copied()
        .find(|macro_name| line.contains(macro_name))
}

#[test]
fn app_and_domain_never_write_to_stdout_or_stderr() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let scanned_dirs = [
        manifest_dir.join("src").join("app"),
        manifest_dir.join("src").join("domain"),
    ];

    let mut files = Vec::new();
    for dir in &scanned_dirs {
        assert!(dir.is_dir(), "expected directory at {}", dir.display());
        collect_rs_files(dir, &mut files);
    }
    files.sort();
    assert!(
        !files.is_empty(),
        "found no .rs files under src/app or src/domain"
    );

    let mut violations: Vec<String> = Vec::new();

    for file in &files {
        let contents = fs::read_to_string(file)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", file.display()));

        for (idx, raw_line) in contents.lines().enumerate() {
            let trimmed = raw_line.trim_start();

            // Skip comment lines: the modules intentionally NAME these macros in
            // doc comments, so only non-comment code is scanned.
            if is_comment_line(trimmed) {
                continue;
            }

            if let Some(macro_name) = first_forbidden_macro(raw_line) {
                violations.push(format!(
                    "{}:{}: presentation macro `{}` in core code: `{}`",
                    file.display(),
                    idx + 1,
                    macro_name,
                    raw_line.trim()
                ));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "Property 4 violated — app/domain code must never call \
         `println!`/`print!`/`eprintln!`/`eprint!` (route output through \
         `PresenterPort`). Offending lines:\n{}",
        violations.join("\n")
    );
}
