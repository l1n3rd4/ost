//! Automated architecture check — Property 5: Single composition root.
//!
//! Validates: Requirements 8.1, 8.2, 8.3, 8.4, 8.5
//!
//! Formal property (design.md):
//!   ∀ m ∈ modules :
//!       constructs(m, {Reqwest*, Toml*, Tungstenite*, ConsolePresenter, TuiPresenter})
//!       ⇒ m = main.rs.
//!   ∧ ∀ svc ∈ App : ¬instantiates(svc, ConcreteAdapter).
//!
//! `main.rs` is the sole composition root: it is the only production site that
//! may construct a concrete adapter. Use-case services in `src/app/` receive
//! their collaborators as injected `Arc<dyn Port>` and must never build a
//! concrete adapter themselves. This test source-scans every `.rs` file under
//! `src/` (except `main.rs`) and fails if any *production* code constructs one
//! of the forbidden concrete adapters.
//!
//! ## What counts as a construction
//!
//! A construction is a constructor *call* that builds a concrete adapter value:
//! `ReqwestTeamsApi::new(...)`, `TomlConfigRepository::default()`,
//! `TomlConfigRepository::new(...)`, `TungsteniteRealtime::new(...)`,
//! `ConsolePresenter::new()`, `TuiPresenter::new(...)`. Every adapter in the
//! codebase is built through such an associated `::new` / `::default` function,
//! so the constructor-call shape is the reliable, false-positive-free signal.
//!
//! Definitions and trait wiring are NOT constructions and must not trip the
//! check: `pub struct ReqwestTeamsApi { .. }`, `impl ReqwestTeamsApi`,
//! `impl TeamsApiPort for ReqwestTeamsApi`, `use adapters::...`, and the unit
//! struct's own `fn new() -> Self { ConsolePresenter }` body all *name* the
//! type without another module *calling* its constructor. Matching the
//! `::new`/`::default` call shape — not the bare type name — is what draws that
//! line.
//!
//! ## Why comment lines are stripped
//!
//! The port and adapter modules intentionally NAME these adapters inside doc
//! comments (e.g. `ports/presenter.rs` documents that "`ConsolePresenter` for
//! the CLI, `TuiPresenter` for the TUI" is chosen at the composition root).
//! Those mentions are documentation, not constructions, so — as with the P3 and
//! P4 checks — every line whose trimmed content begins with `//` (covering
//! `//!` inner-doc, `///` outer-doc, and plain `//` comments) is removed before
//! the scan.
//!
//! ## Why `#[cfg(test)]` code is exempt
//!
//! Property 5 constrains *production* wiring. Adapter unit tests legitimately
//! build their own adapter under test (e.g. `console.rs` does
//! `ConsolePresenter::new()` and `toml_repo.rs` does `TomlConfigRepository::new(..)`
//! inside `#[cfg(test)] mod tests`). Those live behind `#[cfg(test)]` and never
//! ship in the binary, so lines inside a `#[cfg(test)]` module are skipped. The
//! scanner tracks brace depth to find where the test module ends.

use std::fs;
use std::path::{Path, PathBuf};

/// Constructor-call forms of the forbidden concrete adapters.
///
/// Each entry is the prefix of a construction *expression*. The `Reqwest`,
/// `Toml`, and `Tungstenite` wildcards from the design collapse to the concrete
/// constructor calls their adapters expose (`::new` / `::default`), and the two
/// presenters are matched by the same call shape.
///
/// Matching the `::new` / `::default` call shape — rather than the bare type
/// name — is what lets the adapter *definition* modules (`struct`, `impl`,
/// `impl Trait for`, and each adapter's own `fn new` body) pass while their
/// *construction* anywhere but `main.rs` fails.
const FORBIDDEN_CONSTRUCTIONS: &[&str] = &[
    "ReqwestTeamsApi::new",
    "ReqwestTeamsApi::default",
    "TomlConfigRepository::new",
    "TomlConfigRepository::default",
    "TungsteniteRealtime::new",
    "TungsteniteRealtime::default",
    "ConsolePresenter::new",
    "ConsolePresenter::default",
    "TuiPresenter::new",
    "TuiPresenter::default",
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

/// Return the first forbidden construction present on `code_line`, if any.
///
/// Matches the explicit constructor-call forms (`Type::new` / `Type::default`).
/// This is the shape every adapter is actually built through, so it flags real
/// construction sites without tripping on definitions or trait wiring.
fn first_forbidden_construction(code_line: &str) -> Option<&'static str> {
    FORBIDDEN_CONSTRUCTIONS
        .iter()
        .copied()
        .find(|form| code_line.contains(form))
}

#[test]
fn only_main_constructs_concrete_adapters() {
    let src_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    assert!(
        src_dir.is_dir(),
        "expected source directory at {}",
        src_dir.display()
    );

    let main_rs = src_dir.join("main.rs");

    let mut files = Vec::new();
    collect_rs_files(&src_dir, &mut files);
    files.retain(|f| f != &main_rs); // main.rs is the allowed composition root.
    files.sort();
    assert!(
        !files.is_empty(),
        "found no .rs files under {} (excluding main.rs)",
        src_dir.display()
    );

    let mut violations: Vec<String> = Vec::new();

    for file in &files {
        let contents = fs::read_to_string(file)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", file.display()));

        // Track `#[cfg(test)]` module scope by brace depth. When we see a
        // `#[cfg(test)]` attribute we arm a pending flag; the next `{` opens the
        // test module and records its depth. Lines are exempt while inside it.
        let mut brace_depth: i32 = 0;
        let mut pending_cfg_test = false;
        let mut test_module_depth: Option<i32> = None;

        for (idx, raw_line) in contents.lines().enumerate() {
            let trimmed = raw_line.trim_start();

            // Comment lines never construct anything; they also must not affect
            // brace bookkeeping, so skip them entirely.
            if is_comment_line(trimmed) {
                continue;
            }

            // Arm the test-module flag on a `#[cfg(test)]` attribute.
            if test_module_depth.is_none() && trimmed.contains("#[cfg(test)]") {
                pending_cfg_test = true;
            }

            let opens = raw_line.matches('{').count() as i32;
            let closes = raw_line.matches('}').count() as i32;

            // If a `#[cfg(test)]` is pending and this line opens a block, that
            // block is the test module; remember the depth it sits at.
            if pending_cfg_test && opens > 0 {
                test_module_depth = Some(brace_depth);
                pending_cfg_test = false;
            }

            let inside_test_module = test_module_depth.is_some();

            if !inside_test_module {
                if let Some(construction) = first_forbidden_construction(raw_line) {
                    violations.push(format!(
                        "{}:{}: concrete-adapter construction `{}` outside main.rs: `{}`",
                        file.display(),
                        idx + 1,
                        construction,
                        raw_line.trim()
                    ));
                }
            }

            brace_depth += opens - closes;

            // Once brace depth falls back to the test module's opening depth,
            // the `#[cfg(test)]` module has closed.
            if let Some(depth) = test_module_depth {
                if brace_depth <= depth {
                    test_module_depth = None;
                }
            }
        }
    }

    assert!(
        violations.is_empty(),
        "Property 5 violated — only `main.rs` (the single composition root) may \
         construct concrete adapters ({{Reqwest*, Toml*, Tungstenite*, \
         ConsolePresenter, TuiPresenter}}); use-case services must receive ports \
         by injection. Offending lines:\n{}",
        violations.join("\n")
    );
}
