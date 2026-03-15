use std::path::Path;

use pm_strategy::config::StrategyConfig;
use rtt_core::{AssetId, MarketId, SourceId, SourceKind};
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
    #[serde(default)]
    pub health: HealthConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthConfig {
    #[serde(default = "default_health_enabled")]
    pub enabled: bool,
    #[serde(default = "default_health_port")]
    pub port: u16,
}

impl Default for HealthConfig {
    fn default() -> Self {
        Self {
            enabled: default_health_enabled(),
            port: default_health_port(),
        }
    }
}

fn default_health_enabled() -> bool {
    true
}
fn default_health_port() -> u16 {
    9090
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
    #[serde(default)]
    pub asset_ids: Vec<AssetId>,
    #[serde(default)]
    pub market_universe: Option<MarketUniverseConfig>,
    #[serde(default)]
    pub source_bindings: Vec<SourceBindingConfig>,
    #[serde(default = "default_ws_channel_capacity")]
    pub ws_channel_capacity: usize,
    #[serde(default = "default_snapshot_channel_capacity")]
    pub snapshot_channel_capacity: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketUniverseConfig {
    #[serde(default)]
    pub mode: MarketUniverseMode,
    #[serde(default)]
    pub source_id: Option<SourceId>,
    #[serde(default)]
    pub market_ids: Vec<MarketId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MarketUniverseMode {
    #[default]
    Static,
    Discovery,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceBindingConfig {
    pub source_id: SourceId,
    pub source_kind: SourceKind,
    #[serde(default)]
    pub asset_ids: Vec<AssetId>,
    #[serde(default)]
    pub market_ids: Vec<MarketId>,
    #[serde(default)]
    pub instrument_ids: Vec<String>,
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
    #[serde(default = "default_state_file")]
    pub state_file: String,
    #[serde(default = "default_journal_db_path")]
    pub journal_db_path: String,
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
    #[serde(default)]
    pub alert_webhook_url: Option<String>,
}

impl Default for SafetyConfig {
    fn default() -> Self {
        Self {
            max_orders: default_max_orders(),
            max_usd_exposure: default_max_usd_exposure(),
            max_triggers_per_second: default_max_triggers_per_second(),
            require_confirmation: default_require_confirmation(),
            alert_webhook_url: None,
        }
    }
}

fn default_max_orders() -> u64 {
    5
}
fn default_max_usd_exposure() -> f64 {
    10.0
}
fn default_max_triggers_per_second() -> u64 {
    2
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
fn default_state_file() -> String {
    "state.json".to_string()
}
fn default_journal_db_path() -> String {
    "logs/pm-executor.sqlite3".to_string()
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
        if let Some(v) = first_env_value(&["POLY_API_SECRET", "POLY_SECRET"]) {
            self.credentials.api_secret = v;
        }
        if let Ok(v) = std::env::var("POLY_PASSPHRASE") {
            self.credentials.passphrase = v;
        }
        if let Ok(v) = std::env::var("POLY_PRIVATE_KEY") {
            self.credentials.private_key = v;
        }
        if let Some(v) = first_env_value(&["POLY_MAKER_ADDRESS", "POLY_PROXY_ADDRESS"]) {
            self.credentials.maker_address = v;
        }
        if let Some(v) = first_env_value(&["POLY_SIGNER_ADDRESS", "POLY_ADDRESS"]) {
            self.credentials.signer_address = v;
        }
        if let Ok(v) = std::env::var("POLY_ALERT_WEBHOOK_URL") {
            self.safety.alert_webhook_url = Some(v);
        }
    }

    pub fn resolved_subscription_asset_ids(&self) -> Vec<String> {
        let mut asset_ids = self.websocket.asset_ids.clone();

        for binding in &self.websocket.source_bindings {
            if binding.source_kind == SourceKind::PolymarketWs {
                for asset_id in &binding.asset_ids {
                    push_unique_asset_id(&mut asset_ids, asset_id.clone());
                }
            }
        }

        if !self.strategy.token_id.is_empty() {
            push_unique_asset_id(&mut asset_ids, AssetId::new(self.strategy.token_id.clone()));
        }

        asset_ids
            .into_iter()
            .map(|asset_id| asset_id.to_string())
            .collect()
    }
}

fn first_env_value(names: &[&str]) -> Option<String> {
    names
        .iter()
        .find_map(|name| std::env::var(name).ok().filter(|value| !value.is_empty()))
}

fn push_unique_asset_id(asset_ids: &mut Vec<AssetId>, candidate: AssetId) {
    if !asset_ids.iter().any(|existing| existing == &candidate) {
        asset_ids.push(candidate);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

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
        assert_eq!(config.websocket.asset_ids[0].as_str(), "asset1");
        assert_eq!(config.strategy.strategy, "threshold");
        assert_eq!(config.execution.presign_count, 50);
        assert!(config.execution.dry_run);
        assert_eq!(config.execution.journal_db_path, "logs/pm-executor.sqlite3");
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
        assert_eq!(config.execution.journal_db_path, "logs/pm-executor.sqlite3");
        assert_eq!(config.logging.level, "info");
    }

    #[test]
    fn env_var_overrides_config() {
        let _guard = env_lock().lock().unwrap();
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
    fn legacy_env_var_names_override_config() {
        let _guard = env_lock().lock().unwrap();
        let config_toml = r#"
[credentials]
api_secret = "from_file_secret"
maker_address = "from_file_maker"
signer_address = "from_file_signer"

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
        std::env::set_var("POLY_SECRET", "legacy_secret");
        std::env::set_var("POLY_PROXY_ADDRESS", "0xlegacyproxy");
        std::env::set_var("POLY_ADDRESS", "0xlegacysigner");
        let mut config: ExecutorConfig = toml::from_str(config_toml).unwrap();
        config.apply_env_overrides();
        assert_eq!(config.credentials.api_secret, "legacy_secret");
        assert_eq!(config.credentials.maker_address, "0xlegacyproxy");
        assert_eq!(config.credentials.signer_address, "0xlegacysigner");
        std::env::remove_var("POLY_SECRET");
        std::env::remove_var("POLY_PROXY_ADDRESS");
        std::env::remove_var("POLY_ADDRESS");
    }

    #[test]
    fn canonical_env_var_names_win_over_legacy_aliases() {
        let _guard = env_lock().lock().unwrap();
        let config_toml = r#"
[credentials]

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
        std::env::set_var("POLY_SECRET", "legacy_secret");
        std::env::set_var("POLY_API_SECRET", "canonical_secret");
        std::env::set_var("POLY_PROXY_ADDRESS", "0xlegacyproxy");
        std::env::set_var("POLY_MAKER_ADDRESS", "0xcanonicalmaker");
        std::env::set_var("POLY_ADDRESS", "0xlegacysigner");
        std::env::set_var("POLY_SIGNER_ADDRESS", "0xcanonicalsigner");
        let mut config: ExecutorConfig = toml::from_str(config_toml).unwrap();
        config.apply_env_overrides();
        assert_eq!(config.credentials.api_secret, "canonical_secret");
        assert_eq!(config.credentials.maker_address, "0xcanonicalmaker");
        assert_eq!(config.credentials.signer_address, "0xcanonicalsigner");
        std::env::remove_var("POLY_SECRET");
        std::env::remove_var("POLY_API_SECRET");
        std::env::remove_var("POLY_PROXY_ADDRESS");
        std::env::remove_var("POLY_MAKER_ADDRESS");
        std::env::remove_var("POLY_ADDRESS");
        std::env::remove_var("POLY_SIGNER_ADDRESS");
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
    fn journal_db_path_parses_custom_value() {
        let toml_with_custom_path = r#"
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
journal_db_path = "var/test-journal.sqlite3"
[logging]
"#;
        let config: ExecutorConfig = toml::from_str(toml_with_custom_path).unwrap();
        assert_eq!(config.execution.journal_db_path, "var/test-journal.sqlite3");
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
        assert_eq!(config.safety.max_orders, 5);
        assert!((config.safety.max_usd_exposure - 10.0).abs() < 0.01);
        assert_eq!(config.safety.max_triggers_per_second, 2);
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

    #[test]
    fn market_universe_and_source_bindings_parse_with_legacy_shape_intact() {
        let config_toml = r#"
[credentials]
[connection]
[websocket]
asset_ids = ["legacy-asset"]
ws_channel_capacity = 512
snapshot_channel_capacity = 128

[websocket.market_universe]
mode = "discovery"
source_id = "gamma-primary"
market_ids = ["market-1", "market-2"]

[[websocket.source_bindings]]
source_id = "polymarket-public"
source_kind = "polymarket_ws"
asset_ids = ["bound-yes", "bound-no"]
market_ids = ["market-1"]

[[websocket.source_bindings]]
source_id = "reference-mid"
source_kind = "external_reference"
instrument_ids = ["BTC-USD"]

[strategy]
strategy = "threshold"
token_id = "strategy-asset"
side = "Buy"
size = "10"
order_type = "FOK"

[strategy.params]
threshold = 0.45

[execution]
[logging]
"#;

        let config: ExecutorConfig = toml::from_str(config_toml).unwrap();

        assert_eq!(config.websocket.asset_ids[0].as_str(), "legacy-asset");
        let market_universe = config.websocket.market_universe.as_ref().unwrap();
        assert_eq!(market_universe.mode, MarketUniverseMode::Discovery);
        assert_eq!(
            market_universe.source_id.as_ref().unwrap().to_string(),
            "gamma-primary"
        );
        assert_eq!(market_universe.market_ids[1].as_str(), "market-2");
        assert_eq!(config.websocket.source_bindings.len(), 2);
        assert_eq!(
            config.websocket.source_bindings[0].source_kind,
            SourceKind::PolymarketWs
        );
        assert_eq!(
            config.websocket.source_bindings[1].instrument_ids,
            vec!["BTC-USD".to_string()]
        );
    }

    #[test]
    fn resolved_subscription_assets_merge_legacy_bindings_and_strategy_target() {
        let config_toml = r#"
[credentials]
[connection]
[websocket]
asset_ids = ["legacy-asset", "bound-yes"]

[[websocket.source_bindings]]
source_id = "polymarket-public"
source_kind = "polymarket_ws"
asset_ids = ["bound-yes", "bound-no"]

[[websocket.source_bindings]]
source_id = "reference-mid"
source_kind = "external_reference"
instrument_ids = ["BTC-USD"]

[strategy]
strategy = "threshold"
token_id = "strategy-asset"
side = "Buy"
size = "10"
order_type = "FOK"

[strategy.params]
threshold = 0.45

[execution]
[logging]
"#;

        let config: ExecutorConfig = toml::from_str(config_toml).unwrap();

        assert_eq!(
            config.resolved_subscription_asset_ids(),
            vec![
                "legacy-asset".to_string(),
                "bound-yes".to_string(),
                "bound-no".to_string(),
                "strategy-asset".to_string()
            ]
        );
    }
}
