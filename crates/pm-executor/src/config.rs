use std::path::Path;

use pm_strategy::config::StrategyConfig;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutorConfig {
    pub credentials: CredentialsConfig,
    pub connection: ConnectionConfig,
    pub websocket: WebSocketConfig,
    pub strategy: StrategyConfig,
    pub execution: ExecutionConfig,
    pub logging: LoggingConfig,
    #[serde(default)]
    pub safety: SafetyConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CredentialsConfig {
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub api_secret: String,
    #[serde(default)]
    pub passphrase: String,
    #[serde(default)]
    pub private_key: String,
    #[serde(default)]
    pub maker_address: String,
    #[serde(default)]
    pub signer_address: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionConfig {
    #[serde(default = "default_pool_size")]
    pub pool_size: usize,
    #[serde(default = "default_address_family")]
    pub address_family: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebSocketConfig {
    pub asset_ids: Vec<String>,
    #[serde(default = "default_ws_channel_capacity")]
    pub ws_channel_capacity: usize,
    #[serde(default = "default_snapshot_channel_capacity")]
    pub snapshot_channel_capacity: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionConfig {
    #[serde(default = "default_presign_count")]
    pub presign_count: usize,
    #[serde(default)]
    pub is_neg_risk: bool,
    #[serde(default)]
    pub fee_rate_bps: u64,
    #[serde(default = "default_dry_run")]
    pub dry_run: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    #[serde(default = "default_log_level")]
    pub level: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SafetyConfig {
    #[serde(default = "default_max_orders")]
    pub max_orders: u64,
    #[serde(default = "default_max_usd_exposure")]
    pub max_usd_exposure: f64,
    #[serde(default = "default_max_triggers_per_second")]
    pub max_triggers_per_second: u64,
    #[serde(default = "default_require_confirmation")]
    pub require_confirmation: bool,
}

impl Default for SafetyConfig {
    fn default() -> Self {
        Self {
            max_orders: default_max_orders(),
            max_usd_exposure: default_max_usd_exposure(),
            max_triggers_per_second: default_max_triggers_per_second(),
            require_confirmation: default_require_confirmation(),
        }
    }
}

fn default_max_orders() -> u64 {
    10
}
fn default_max_usd_exposure() -> f64 {
    50.0
}
fn default_max_triggers_per_second() -> u64 {
    1
}
fn default_require_confirmation() -> bool {
    true
}

fn default_pool_size() -> usize {
    2
}
fn default_address_family() -> String {
    "auto".to_string()
}
fn default_ws_channel_capacity() -> usize {
    1024
}
fn default_snapshot_channel_capacity() -> usize {
    256
}
fn default_presign_count() -> usize {
    100
}
fn default_dry_run() -> bool {
    true
}
fn default_log_level() -> String {
    "info".to_string()
}

impl ExecutorConfig {
    pub fn load(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let content = std::fs::read_to_string(path)?;
        let mut config: Self = toml::from_str(&content)?;
        config.apply_env_overrides();
        Ok(config)
    }

    fn apply_env_overrides(&mut self) {
        if let Ok(v) = std::env::var("POLY_API_KEY") {
            self.credentials.api_key = v;
        }
        if let Ok(v) = std::env::var("POLY_API_SECRET") {
            self.credentials.api_secret = v;
        }
        if let Ok(v) = std::env::var("POLY_PASSPHRASE") {
            self.credentials.passphrase = v;
        }
        if let Ok(v) = std::env::var("POLY_PRIVATE_KEY") {
            self.credentials.private_key = v;
        }
        if let Ok(v) = std::env::var("POLY_MAKER_ADDRESS") {
            self.credentials.maker_address = v;
        }
        if let Ok(v) = std::env::var("POLY_SIGNER_ADDRESS") {
            self.credentials.signer_address = v;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID_TOML: &str = r#"
[credentials]
api_key = "test_key"
api_secret = "test_secret"
passphrase = "test_pass"
private_key = "0xdeadbeef"
maker_address = "0xmaker"
signer_address = "0xsigner"

[connection]
pool_size = 4
address_family = "ipv4"

[websocket]
asset_ids = ["asset1", "asset2"]
ws_channel_capacity = 512
snapshot_channel_capacity = 128

[strategy]
strategy = "threshold"
token_id = "asset1"
side = "Buy"
size = "10"
order_type = "FOK"

[strategy.params]
threshold = 0.45

[execution]
presign_count = 50
is_neg_risk = false
fee_rate_bps = 0
dry_run = true

[logging]
level = "debug"
"#;

    #[test]
    fn parse_valid_config() {
        let config: ExecutorConfig = toml::from_str(VALID_TOML).unwrap();
        assert_eq!(config.credentials.api_key, "test_key");
        assert_eq!(config.connection.pool_size, 4);
        assert_eq!(config.websocket.asset_ids.len(), 2);
        assert_eq!(config.strategy.strategy, "threshold");
        assert_eq!(config.execution.presign_count, 50);
        assert!(config.execution.dry_run);
        assert_eq!(config.logging.level, "debug");
    }

    #[test]
    fn defaults_applied_for_optional_fields() {
        let minimal_toml = r#"
[credentials]

[connection]

[websocket]
asset_ids = ["asset1"]

[strategy]
strategy = "threshold"
token_id = "asset1"
side = "Buy"
size = "10"
order_type = "FOK"

[strategy.params]
threshold = 0.45

[execution]

[logging]
"#;
        let config: ExecutorConfig = toml::from_str(minimal_toml).unwrap();
        assert_eq!(config.connection.pool_size, 2);
        assert_eq!(config.connection.address_family, "auto");
        assert_eq!(config.websocket.ws_channel_capacity, 1024);
        assert_eq!(config.execution.presign_count, 100);
        assert_eq!(config.logging.level, "info");
    }

    #[test]
    fn env_var_overrides_config() {
        let config_toml = r#"
[credentials]
api_key = "from_file"

[connection]
[websocket]
asset_ids = ["a"]
[strategy]
strategy = "threshold"
token_id = "a"
side = "Buy"
size = "1"
order_type = "FOK"
[strategy.params]
threshold = 0.5
[execution]
[logging]
"#;
        std::env::set_var("POLY_API_KEY", "from_env");
        let mut config: ExecutorConfig = toml::from_str(config_toml).unwrap();
        config.apply_env_overrides();
        assert_eq!(config.credentials.api_key, "from_env");
        std::env::remove_var("POLY_API_KEY");
    }

    #[test]
    fn dry_run_defaults_to_true() {
        let minimal_toml = r#"
[credentials]
[connection]
[websocket]
asset_ids = ["asset1"]
[strategy]
strategy = "threshold"
token_id = "asset1"
side = "Buy"
size = "10"
order_type = "FOK"
[strategy.params]
threshold = 0.45
[execution]
[logging]
"#;
        let config: ExecutorConfig = toml::from_str(minimal_toml).unwrap();
        assert!(config.execution.dry_run, "dry_run should default to true");
    }

    #[test]
    fn dry_run_parses_false() {
        let toml_with_dry_run = r#"
[credentials]
[connection]
[websocket]
asset_ids = ["asset1"]
[strategy]
strategy = "threshold"
token_id = "asset1"
side = "Buy"
size = "10"
order_type = "FOK"
[strategy.params]
threshold = 0.45
[execution]
dry_run = false
[logging]
"#;
        let config: ExecutorConfig = toml::from_str(toml_with_dry_run).unwrap();
        assert!(!config.execution.dry_run);
    }

    #[test]
    fn strategy_builds_from_config() {
        let config: ExecutorConfig = toml::from_str(VALID_TOML).unwrap();
        let strategy = config.strategy.build_strategy();
        assert!(strategy.is_ok());
        assert_eq!(strategy.unwrap().name(), "threshold");
    }

    #[test]
    fn safety_defaults_applied_without_section() {
        let minimal_toml = r#"
[credentials]
[connection]
[websocket]
asset_ids = ["asset1"]
[strategy]
strategy = "threshold"
token_id = "asset1"
side = "Buy"
size = "10"
order_type = "FOK"
[strategy.params]
threshold = 0.45
[execution]
[logging]
"#;
        let config: ExecutorConfig = toml::from_str(minimal_toml).unwrap();
        assert_eq!(config.safety.max_orders, 10);
        assert!((config.safety.max_usd_exposure - 50.0).abs() < 0.01);
        assert_eq!(config.safety.max_triggers_per_second, 1);
        assert!(config.safety.require_confirmation);
    }

    #[test]
    fn safety_config_parses_custom_values() {
        let toml_with_safety = r#"
[credentials]
[connection]
[websocket]
asset_ids = ["asset1"]
[strategy]
strategy = "threshold"
token_id = "asset1"
side = "Buy"
size = "10"
order_type = "FOK"
[strategy.params]
threshold = 0.45
[execution]
[safety]
max_orders = 20
max_usd_exposure = 100.0
max_triggers_per_second = 5
require_confirmation = false
[logging]
"#;
        let config: ExecutorConfig = toml::from_str(toml_with_safety).unwrap();
        assert_eq!(config.safety.max_orders, 20);
        assert!((config.safety.max_usd_exposure - 100.0).abs() < 0.01);
        assert_eq!(config.safety.max_triggers_per_second, 5);
        assert!(!config.safety.require_confirmation);
    }
}
