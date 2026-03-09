use std::path::Path;

use serde::{Deserialize, Serialize};

/// Persisted executor state — written on shutdown, read on startup.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct ExecutorState {
    pub orders_fired: u64,
    pub usd_committed_cents: u64,
    pub last_shutdown: String,
    pub tripped: bool,
}

impl ExecutorState {
    /// Load state from a JSON file. Returns Default if file is missing or corrupt.
    pub fn load(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    /// Save state to a JSON file. Creates parent dirs if needed.
    pub fn save(&self, path: &Path) -> Result<(), std::io::Error> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        std::fs::write(path, json)
    }

    /// Build from current circuit breaker stats.
    pub fn from_stats(orders_fired: u64, usd_committed_cents: u64, tripped: bool) -> Self {
        let now = chrono::Utc::now().to_rfc3339();
        Self {
            orders_fired,
            usd_committed_cents,
            last_shutdown: now,
            tripped,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("rtt_test_state_{}", name))
    }

    #[test]
    fn save_and_load_roundtrip() {
        let path = temp_path("roundtrip.json");
        let state = ExecutorState {
            orders_fired: 3,
            usd_committed_cents: 1500,
            last_shutdown: "2026-03-07T12:00:00Z".to_string(),
            tripped: false,
        };
        state.save(&path).unwrap();
        let loaded = ExecutorState::load(&path);
        assert_eq!(loaded, state);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn load_missing_file_returns_default() {
        let path = temp_path("nonexistent_xyz.json");
        let _ = std::fs::remove_file(&path); // ensure it doesn't exist
        let state = ExecutorState::load(&path);
        assert_eq!(state, ExecutorState::default());
    }

    #[test]
    fn load_corrupt_file_returns_default() {
        let path = temp_path("corrupt.json");
        std::fs::write(&path, "not json at all {{{").unwrap();
        let state = ExecutorState::load(&path);
        assert_eq!(state, ExecutorState::default());
        let _ = std::fs::remove_file(&path);
    }
}
