//! Presenter adapters (ports & adapters).
//!
//! Concrete [`PresenterPort`](crate::ports::PresenterPort) implementations that
//! render domain models to a presentation surface. `ConsolePresenter` writes to
//! stdout/stderr for the CLI; `TuiPresenter` feeds domain models into TUI state
//! over a channel instead of printing. Presenters are the one place where
//! `println!`/`eprintln!` are allowed, keeping the app and domain layers free of
//! presentation concerns.

pub mod console;
pub mod tui;

pub use console::ConsolePresenter;
pub use tui::{TuiPresenter, TuiUpdate};
