//! TOML-backed config repository adapter.
//!
//! Implements [`ConfigRepositoryPort`] over a TOML file on the local
//! filesystem. This is an *adapter*: `toml` and `std::fs` are allowed here but
//! kept out of any public signature. All filesystem and parse failures are
//! mapped to [`DomainError::Storage`], so callers see only domain types.
//!
//! The domain [`Credentials`] aggregate derives `Serialize`/`Deserialize`, so
//! it is (de)serialized directly to/from TOML with no intermediate mapping.

use std::fs;
use std::path::PathBuf;

use directories::ProjectDirs;

use crate::domain::{Credentials, DomainError, DomainResult};
use crate::ports::ConfigRepositoryPort;

/// Persists [`Credentials`] as a TOML file under the platform config directory.
///
/// On Unix the file is written with `0o600` permissions since it holds token
/// material.
#[derive(Debug, Clone)]
pub struct TomlConfigRepository {
    path: PathBuf,
}

impl TomlConfigRepository {
    /// Build a repository writing to the given file path.
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Resolve the default config directory (`com/teams-cli/teams-cli`).
    fn default_config_dir() -> DomainResult<PathBuf> {
        ProjectDirs::from("com", "teams-cli", "teams-cli")
            .map(|dirs| dirs.config_dir().to_path_buf())
            .ok_or_else(|| {
                DomainError::Storage("could not determine config directory".to_string())
            })
    }
}

impl Default for TomlConfigRepository {
    /// Resolve the default config path (`<config_dir>/config.toml`).
    ///
    /// Mirrors the resolution used by the legacy `Config` type so the adapter
    /// reads and writes the same location.
    fn default() -> Self {
        // Fall back to a bare relative path if the platform config dir cannot
        // be resolved; `load`/`save` then surface any real failure as Storage.
        let path = Self::default_config_dir()
            .map(|dir| dir.join("config.toml"))
            .unwrap_or_else(|_| PathBuf::from("config.toml"));
        Self::new(path)
    }
}

impl ConfigRepositoryPort for TomlConfigRepository {
    fn load(&self) -> DomainResult<Credentials> {
        if !self.path.exists() {
            return Ok(Credentials::default());
        }

        let content = fs::read_to_string(&self.path)
            .map_err(|e| DomainError::Storage(format!("failed to read config file: {e}")))?;

        toml::from_str(&content)
            .map_err(|e| DomainError::Storage(format!("failed to parse config file: {e}")))
    }

    fn save(&self, creds: &Credentials) -> DomainResult<()> {
        // Serialize first: a serialization failure must not touch the existing
        // file, preserving prior content.
        let content = toml::to_string_pretty(creds)
            .map_err(|e| DomainError::Storage(format!("failed to serialize config: {e}")))?;

        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir).map_err(|e| {
                DomainError::Storage(format!("failed to create config directory: {e}"))
            })?;
        }

        fs::write(&self.path, content)
            .map_err(|e| DomainError::Storage(format!("failed to write config file: {e}")))?;

        // Restrict permissions: the file holds token material.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = fs::Permissions::from_mode(0o600);
            fs::set_permissions(&self.path, perms).map_err(|e| {
                DomainError::Storage(format!("failed to set config permissions: {e}"))
            })?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::StoredToken;

    /// RAII temp directory under the system temp dir with a unique name.
    /// Removed (best-effort) on drop so tests leave no residue. Uses only
    /// `std::fs` and `uuid` — no network access anywhere in these tests.
    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "teams-cli-config-test-{}",
                uuid::Uuid::new_v4()
            ));
            fs::create_dir_all(&path).expect("failed to create temp dir");
            Self { path }
        }

        /// Path to a `config.toml` inside this temp dir (not created).
        fn config_path(&self) -> PathBuf {
            self.path.join("config.toml")
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn load_missing_file_returns_default() {
        let temp = TempDir::new();
        let repo = TomlConfigRepository::new(temp.config_path());

        let creds = repo.load().expect("load of missing file should succeed");

        // Default credentials have every field unset.
        assert!(creds.access_token.is_none());
        assert!(creds.refresh_token.is_none());
        assert!(creds.skype_token.is_none());
        assert!(creds.graph_token.is_none());
        assert!(creds.ic3_token.is_none());
        assert!(creds.recorder_token.is_none());
        assert!(creds.tenant_id.is_none());
        assert!(creds.region_gtms.is_none());
    }

    #[test]
    fn save_then_load_round_trips_credentials() {
        let temp = TempDir::new();
        let repo = TomlConfigRepository::new(temp.config_path());

        let creds = Credentials {
            access_token: Some(StoredToken {
                token: "abc".to_string(),
                expires_at: Some(123),
            }),
            refresh_token: Some("refresh-xyz".to_string()),
            skype_token: None,
            graph_token: Some(StoredToken {
                token: "graph".to_string(),
                expires_at: None,
            }),
            ic3_token: None,
            recorder_token: None,
            tenant_id: Some("t1".to_string()),
            region_gtms: Some("us".to_string()),
        };

        repo.save(&creds).expect("save should succeed");

        let loaded = repo.load().expect("load should succeed");

        let access = loaded.access_token.expect("access_token should be present");
        assert_eq!(access.token, "abc");
        assert_eq!(access.expires_at, Some(123));
        assert_eq!(loaded.refresh_token.as_deref(), Some("refresh-xyz"));
        assert!(loaded.skype_token.is_none());
        let graph = loaded.graph_token.expect("graph_token should be present");
        assert_eq!(graph.token, "graph");
        assert_eq!(graph.expires_at, None);
        assert!(loaded.ic3_token.is_none());
        assert!(loaded.recorder_token.is_none());
        assert_eq!(loaded.tenant_id.as_deref(), Some("t1"));
        assert_eq!(loaded.region_gtms.as_deref(), Some("us"));
    }
}
