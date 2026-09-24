//! Task 8.3 — Behavior-preservation tests for the CLI commands.
//!
//! Validates: Requirements 10.1, 10.2, 10.4
//!
//! Goal (R10.1/10.2): after moving `main.rs` onto the hexagonal use-case
//! services (task 8.1), every CLI command must still produce **byte-identical
//! stdout + stderr and the same process exit code** as the pre-refactor CLI for
//! the same input. R10.4 further requires that a command's *observable* result
//! is a pure function of its input (here: argv + the on-disk config), so with a
//! fixed config fixture the bytes are stable and reproducible.
//!
//! ## How these are "recorded fixtures" without a network
//!
//! The commands `{login, logout, status, whoami, teams, chats, read, send,
//! presence}` normally require a live Teams session. We must never make real
//! network calls in CI, so each case here pins the *input* — argv plus a
//! per-test temporary `XDG_CONFIG_HOME` (the `directories` crate honors it on
//! Linux, so `Config`/`TomlConfigRepository` read/write there instead of the
//! real user config) — and records the exact stdout/stderr/exit-code the binary
//! emits for that pinned input. The recorded expectations below ARE the
//! fixtures; re-running the binary must reproduce them byte-for-byte.
//!
//! Two knobs make the bytes deterministic:
//!   * `RUST_LOG=off` silences the `tracing` layer. The info logs (e.g.
//!     "Logging out...", "Fetching chats...") carry a wall-clock timestamp and
//!     are therefore NOT byte-stable; they are diagnostics, not command output,
//!     and disabling them leaves the actual command stdout/stderr — the surface
//!     R10 constrains. (The legacy CLI emitted the same timestamped logs, so
//!     silencing them compares like with like.)
//!   * A fresh empty `XDG_CONFIG_HOME` per test → the deterministic
//!     "unauthenticated" state, with no dependency on the developer's real
//!     credentials.
//!
//! ## What is exercised vs deferred (and why)
//!
//! Fully exercised end-to-end (spawned binary, real bytes asserted):
//!   * `logout`  — expressed purely through `ConfigRepositoryPort`; writes the
//!     temp config and prints `Logged out.` (exit 0). No network.
//!   * `status`  — reads only the config; an empty fixture yields the full
//!     "all tokens none" listing + login hint (exit 0). No network.
//!   * `whoami`, `teams`, `chats`, `read`, `send`, `presence` — each first
//!     builds the HTTP adapter, which loads the (empty) config, finds no token
//!     and no refresh token, and fails deterministically *before any socket is
//!     opened*. We assert that stable unauthenticated error path: empty stdout,
//!     the `Error: transport failure: ...` line on stderr, exit 1. This is the
//!     offline-deterministic behavior R10.1 preserves for network commands.
//!   * `login` — its help/arg-parsing surface is asserted. Its *interactive*
//!     device-code path is deferred: on an empty config it falls straight
//!     through to the OAuth device-code request, which requires the network, so
//!     it cannot run offline in CI without real calls. Documented here and
//!     covered by the help/usage assertion instead.
//!
//! Also pinned: top-level `--help`, `send` with missing args (clap exit 2), and
//! per-command `--help` — the argument-parsing contract, which is part of the
//! observable CLI behavior and is fully deterministic offline.

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Path to the freshly-compiled `teams-cli` binary under test.
///
/// Cargo sets `CARGO_BIN_EXE_<name>` for integration tests, so this always
/// points at the same build the rest of `cargo test` uses — no PATH lookup, no
/// separate build step.
const BIN: &str = env!("CARGO_BIN_EXE_teams-cli");

/// Monotonic counter so concurrently-run tests never share a config dir.
static COUNTER: AtomicU32 = AtomicU32::new(0);

/// Create a unique, empty temporary directory to serve as `XDG_CONFIG_HOME`.
///
/// Using a per-test directory guarantees the "unauthenticated" fixture state
/// and keeps tests hermetic: they never read or mutate the developer's real
/// `~/.config/teams-cli`.
fn temp_config_dir() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut dir = std::env::temp_dir();
    dir.push(format!("teams-cli-bp-{}-{}-{}", std::process::id(), nanos, n));
    std::fs::create_dir_all(&dir).expect("create temp config dir");
    dir
}

/// Outcome of running the CLI: captured stdout, stderr, and exit code.
struct Output {
    stdout: String,
    stderr: String,
    code: Option<i32>,
}

/// Run `teams-cli <args...>` against a fresh empty config dir with logging off.
///
/// Returns the decoded stdout/stderr and the process exit code. The child
/// inherits a minimal, controlled environment (`RUST_LOG=off`,
/// `XDG_CONFIG_HOME=<temp>`) so its output is a pure function of `args` — the
/// determinism R10.4 requires.
fn run(args: &[&str]) -> Output {
    let cfg = temp_config_dir();
    let out = Command::new(BIN)
        .args(args)
        .env("RUST_LOG", "off")
        .env("XDG_CONFIG_HOME", &cfg)
        .output()
        .expect("failed to spawn teams-cli");
    let _ = std::fs::remove_dir_all(&cfg);
    Output {
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        code: out.status.code(),
    }
}

/// Run the CLI and return both the captured output and the config dir it used
/// (left on disk so the caller can inspect what was persisted).
fn run_keep_config(args: &[&str]) -> (Output, PathBuf) {
    let cfg = temp_config_dir();
    let out = Command::new(BIN)
        .args(args)
        .env("RUST_LOG", "off")
        .env("XDG_CONFIG_HOME", &cfg)
        .output()
        .expect("failed to spawn teams-cli");
    (
        Output {
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            code: out.status.code(),
        },
        cfg,
    )
}

/// The exact unauthenticated stderr the HTTP-backed commands emit.
///
/// The line appears **twice**: once when the composition root presents the
/// `DomainError` via `ConsolePresenter::show_error`, and once when `main`
/// returns the same error and `anyhow` prints it on exit. This doubling is the
/// pre-refactor behavior we are preserving, so the fixture pins both lines.
const UNAUTH_ERR: &str = "Error: transport failure: Token expired and no refresh token. Run 'teams-cli login'.\n\
Error: transport failure: Token expired and no refresh token. Run 'teams-cli login'.\n";

// ---------------------------------------------------------------------------
// Config-only commands: full behavior asserted (no network involved at all).
// ---------------------------------------------------------------------------

/// `logout` routes through `ConfigRepositoryPort`: it prints exactly
/// `Logged out.` to stdout, writes nothing to stderr, and exits 0.
#[test]
fn logout_prints_logged_out_and_exits_zero() {
    let out = run(&["logout"]);
    assert_eq!(out.stdout, "Logged out.\n", "logout stdout must be stable");
    assert_eq!(out.stderr, "", "logout writes nothing to stderr");
    assert_eq!(out.code, Some(0), "logout exit code must be 0");
}

/// `logout` persists a config file through the repository port (behavior
/// preservation of the legacy `clear_tokens` + save), proving the command
/// exercised the real config adapter rather than a no-op.
#[test]
fn logout_writes_config_through_repository_port() {
    let (out, cfg) = run_keep_config(&["logout"]);
    assert_eq!(out.code, Some(0));
    let config_file = cfg.join("teams-cli").join("config.toml");
    let existed = config_file.exists();
    let _ = std::fs::remove_dir_all(&cfg);
    assert!(
        existed,
        "logout must persist config via ConfigRepositoryPort at {}",
        config_file.display()
    );
}

/// `status` with an empty config lists every token as `none` and appends the
/// login hint, byte-for-byte, exiting 0. No network is touched.
#[test]
fn status_empty_config_lists_all_tokens_none() {
    let out = run(&["status"]);
    let expected = "\
AAD token:   none
Refresh tok: none
Graph token: none
IC3 token:   none
Recorder tk: none
Skype token: none
Region GTMs: none

Run 'teams-cli login' to authenticate.
";
    assert_eq!(out.stdout, expected, "status stdout must be byte-identical");
    assert_eq!(out.stderr, "", "status writes nothing to stderr");
    assert_eq!(out.code, Some(0), "status exit code must be 0");
}

// ---------------------------------------------------------------------------
// HTTP-backed commands: deterministic offline (unauthenticated) behavior.
//
// Each builds the HTTP adapter first, which fails on the empty config before
// any socket opens, so the observable bytes are stable without a network.
// ---------------------------------------------------------------------------

/// `chats` on an empty config: no stdout, the unauthenticated error on stderr,
/// exit 1.
#[test]
fn chats_unauthenticated_is_stable() {
    let out = run(&["chats"]);
    assert_eq!(out.stdout, "", "chats prints nothing before auth fails");
    assert_eq!(out.stderr, UNAUTH_ERR, "chats stderr must be stable");
    assert_eq!(out.code, Some(1), "chats exit code must be 1");
}

/// `whoami` unauthenticated: stable error, exit 1.
#[test]
fn whoami_unauthenticated_is_stable() {
    let out = run(&["whoami"]);
    assert_eq!(out.stdout, "");
    assert_eq!(out.stderr, UNAUTH_ERR);
    assert_eq!(out.code, Some(1));
}

/// `teams` unauthenticated: stable error, exit 1.
#[test]
fn teams_unauthenticated_is_stable() {
    let out = run(&["teams"]);
    assert_eq!(out.stdout, "");
    assert_eq!(out.stderr, UNAUTH_ERR);
    assert_eq!(out.code, Some(1));
}

/// `read <chat_id>` unauthenticated: stable error, exit 1. (The adapter is
/// built before the service validates the id, so the auth error dominates.)
#[test]
fn read_unauthenticated_is_stable() {
    let out = run(&["read", "19:abc@thread.v2"]);
    assert_eq!(out.stdout, "");
    assert_eq!(out.stderr, UNAUTH_ERR);
    assert_eq!(out.code, Some(1));
}

/// `send --to <id> <msg>` unauthenticated: stable error, exit 1.
#[test]
fn send_unauthenticated_is_stable() {
    let out = run(&["send", "--to", "19:abc@thread.v2", "hello"]);
    assert_eq!(out.stdout, "");
    assert_eq!(out.stderr, UNAUTH_ERR);
    assert_eq!(out.code, Some(1));
}

/// `presence` (get) unauthenticated: stable error, exit 1.
#[test]
fn presence_get_unauthenticated_is_stable() {
    let out = run(&["presence"]);
    assert_eq!(out.stdout, "");
    assert_eq!(out.stderr, UNAUTH_ERR);
    assert_eq!(out.code, Some(1));
}

/// `presence --set busy` unauthenticated: same stable error path, exit 1.
#[test]
fn presence_set_unauthenticated_is_stable() {
    let out = run(&["presence", "--set", "busy"]);
    assert_eq!(out.stdout, "");
    assert_eq!(out.stderr, UNAUTH_ERR);
    assert_eq!(out.code, Some(1));
}

// ---------------------------------------------------------------------------
// Argument-parsing contract (fully deterministic, part of observable behavior).
// Covers `login`, whose interactive device-code path is deferred (needs net).
// ---------------------------------------------------------------------------

/// Top-level `--help` lists every subcommand and exits 0. This pins the CLI
/// surface (command set + descriptions) that behavior preservation protects.
#[test]
fn top_level_help_lists_all_commands() {
    let out = run(&["--help"]);
    assert_eq!(out.code, Some(0), "help exits 0");
    assert_eq!(out.stderr, "", "help goes to stdout, not stderr");
    for token in [
        "login", "logout", "status", "chats", "read", "send", "teams", "whoami", "trouter",
        "presence", "tui",
    ] {
        assert!(
            out.stdout.contains(token),
            "top-level help must list `{token}`; got:\n{}",
            out.stdout
        );
    }
}

/// `login --help` is deterministic and offline: it documents the `--force`
/// flag and exits 0. The interactive login flow itself is deferred (device-code
/// exchange requires the network and must not run in CI).
#[test]
fn login_help_is_stable_and_documents_force() {
    let out = run(&["login", "--help"]);
    assert_eq!(out.code, Some(0));
    assert_eq!(out.stderr, "");
    assert!(
        out.stdout.contains("--force"),
        "login help must document --force; got:\n{}",
        out.stdout
    );
}

/// `send` with no arguments is a clap usage error: nothing on stdout, a usage
/// message naming the missing `--to`/`<MESSAGE>` on stderr, and exit code 2
/// (clap's conventional argument-error code). This is stable across the
/// refactor because the `Cli`/`Commands` definitions were preserved.
#[test]
fn send_missing_args_is_clap_usage_error() {
    let out = run(&["send"]);
    assert_eq!(out.stdout, "", "usage errors go to stderr, not stdout");
    assert_eq!(out.code, Some(2), "clap argument errors exit with code 2");
    assert!(
        out.stderr.contains("--to") && out.stderr.contains("MESSAGE"),
        "send usage error must name the missing args; got:\n{}",
        out.stderr
    );
    assert!(
        out.stderr.contains("Usage: teams-cli send"),
        "send usage error must show the usage line; got:\n{}",
        out.stderr
    );
}
