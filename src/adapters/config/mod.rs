//! Config adapters.
//!
//! Concrete [`ConfigRepositoryPort`](crate::ports::ConfigRepositoryPort)
//! implementations. Currently a single TOML-file adapter.

pub mod toml_repo;

pub use toml_repo::TomlConfigRepository;
