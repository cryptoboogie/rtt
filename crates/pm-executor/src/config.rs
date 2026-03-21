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
    #[serde(default)]
    pub quote_mode: QuoteModeConfig,
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuoteModeConfig {
    #[serde(default = "default_analysis_db_path")]
    pub analysis_db_path: String,
    #[serde(default = "default_quote_base_url")]
    pub clob_base_url: String,
    #[serde(default = "default_user_ws_url")]
    pub user_ws_url: String,
    #[serde(default = "default_heartbeat_interval_secs")]
    pub heartbeat_interval_secs: u64,
    #[serde(default = "default_reward_poll_interval_secs")]
    pub reward_poll_interval_secs: u64,
    #[serde(default = "default_rebate_poll_interval_secs")]
    pub rebate_poll_interval_secs: u64,
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

impl Default for QuoteModeConfig {
    fn default() -> Self {
        Self {
            analysis_db_path: default_analysis_db_path(),
            clob_base_url: default_quote_base_url(),
            user_ws_url: default_user_ws_url(),
            heartbeat_interval_secs: default_heartbeat_interval_secs(),
            reward_poll_interval_secs: default_reward_poll_interval_secs(),
            rebate_poll_interval_secs: default_rebate_poll_interval_secs(),
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
fn default_analysis_db_path() -> String {
    "analysis.sqlite".to_string()
}
fn default_quote_base_url() -> String {
    rtt_core::polymarket::CLOB_BASE_URL.to_string()
}
fn default_user_ws_url() -> String {
    "wss://ws-subscriptions-clob.polymarket.com/ws/user".to_string()
}
fn default_heartbeat_interval_secs() -> u64 {
    5
}
fn default_reward_poll_interval_secs() -> u64 {
    60
}
fn default_rebate_poll_interval_secs() -> u64 {
    300
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
        override_string("POLY_API_KEY", &mut self.credentials.api_key);
        override_string_alias(
            "POLY_API_SECRET",
            "POLY_SECRET",
            &mut self.credentials.api_secret,
        );
        override_string("POLY_PASSPHRASE", &mut self.credentials.passphrase);
        override_string("POLY_PRIVATE_KEY", &mut self.credentials.private_key);
        override_string_alias(
            "POLY_MAKER_ADDRESS",
            "POLY_PROXY_ADDRESS",
            &mut self.credentials.maker_address,
        );
        override_string_alias(
            "POLY_SIGNER_ADDRESS",
            "POLY_ADDRESS",
            &mut self.credentials.signer_address,
        );
        if let Ok(v) = std::env::var("POLY_ALERT_WEBHOOK_URL") {
            self.safety.alert_webhook_url = Some(v);
        }

        override_string("RTT_STRATEGY", &mut self.strategy.strategy);
        override_string("RTT_TOKEN_ID", &mut self.strategy.token_id);
        override_bool("RTT_DRY_RUN", &mut self.execution.dry_run);
        override_string(
            "RTT_ANALYSIS_DB_PATH",
            &mut self.quote_mode.analysis_db_path,
        );
        override_string("RTT_CLOB_BASE_URL", &mut self.quote_mode.clob_base_url);
        override_string("RTT_USER_WS_URL", &mut self.quote_mode.user_ws_url);
        override_u64(
            "RTT_HEARTBEAT_INTERVAL_SECS",
            &mut self.quote_mode.heartbeat_interval_secs,
        );
        override_u64(
            "RTT_REWARD_POLL_INTERVAL_SECS",
            &mut self.quote_mode.reward_poll_interval_secs,
        );
        override_u64(
            "RTT_REBATE_POLL_INTERVAL_SECS",
            &mut self.quote_mode.rebate_poll_interval_secs,
        );
        override_f64(
            "RTT_INITIAL_BANKROLL_USD",
            &mut self.strategy.params.initial_bankroll_usd,
        );
        override_f64(
            "RTT_MAX_TOTAL_DEPLOYED_USD",
            &mut self.strategy.params.max_total_deployed_usd,
        );
        override_usize("RTT_MAX_MARKETS", &mut self.strategy.params.max_markets);
        override_f64(
            "RTT_BASE_QUOTE_SIZE",
            &mut self.strategy.params.base_quote_size,
        );
        override_f64("RTT_EDGE_BUFFER", &mut self.strategy.params.edge_buffer);
        override_f64(
            "RTT_TARGET_SPREAD_CENTS",
            &mut self.strategy.params.target_spread_cents,
        );
        override_optional_u64(
            "RTT_QUOTE_TTL_SECS",
            &mut self.strategy.params.quote_ttl_secs,
        );
        override_f64(
            "RTT_MIN_TOTAL_DAILY_RATE",
            &mut self.strategy.params.min_total_daily_rate,
        );
        override_f64(
            "RTT_MAX_MARKET_COMPETITIVENESS",
            &mut self.strategy.params.max_market_competitiveness,
        );
        override_optional_u64(
            "RTT_MIN_TIME_TO_EXPIRY_SECS",
            &mut self.strategy.params.min_time_to_expiry_secs,
        );
        override_f64(
            "RTT_MAX_INVENTORY_PER_MARKET",
            &mut self.strategy.params.max_inventory_per_market,
        );
        override_f64(
            "RTT_MAX_UNHEDGED_NOTIONAL_PER_MARKET",
            &mut self.strategy.params.max_unhedged_notional_per_market,
        );
        override_u64("RTT_SAFETY_MAX_ORDERS", &mut self.safety.max_orders);
        override_f64_required(
            "RTT_SAFETY_MAX_USD_EXPOSURE",
            &mut self.safety.max_usd_exposure,
        );
        override_u64(
            "RTT_SAFETY_MAX_TRIGGERS_PER_SECOND",
            &mut self.safety.max_triggers_per_second,
        );
        override_bool(
            "RTT_REQUIRE_CONFIRMATION",
            &mut self.safety.require_confirmation,
        );
        override_string("RTT_LOG_LEVEL", &mut self.logging.level);
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

fn override_string(name: &str, target: &mut String) {
    if let Ok(value) = std::env::var(name) {
        *target = value;
    }
}

fn override_string_alias(primary: &str, fallback: &str, target: &mut String) {
    if let Ok(value) = std::env::var(primary) {
        *target = value;
    } else if let Ok(value) = std::env::var(fallback) {
        *target = value;
    }
}

fn override_bool(name: &str, target: &mut bool) {
    if let Ok(value) = std::env::var(name) {
        if let Ok(parsed) = value.parse::<bool>() {
            *target = parsed;
        }
    }
}

fn override_u64(name: &str, target: &mut u64) {
    if let Ok(value) = std::env::var(name) {
        if let Ok(parsed) = value.parse::<u64>() {
            *target = parsed;
        }
    }
}

fn override_usize(name: &str, target: &mut Option<usize>) {
    if let Ok(value) = std::env::var(name) {
        if let Ok(parsed) = value.parse::<usize>() {
            *target = Some(parsed);
        }
    }
}

fn override_f64(name: &str, target: &mut Option<f64>) {
    if let Ok(value) = std::env::var(name) {
        if let Ok(parsed) = value.parse::<f64>() {
            *target = Some(parsed);
        }
    }
}

fn override_optional_u64(name: &str, target: &mut Option<u64>) {
    if let Ok(value) = std::env::var(name) {
        if let Ok(parsed) = value.parse::<u64>() {
            *target = Some(parsed);
        }
    }
}

fn override_f64_required(name: &str, target: &mut f64) {
    if let Ok(value) = std::env::var(name) {
        if let Ok(parsed) = value.parse::<f64>() {
            *target = parsed;
        }
    }
}

fn push_unique_asset_id(asset_ids: &mut Vec<AssetId>, candidate: AssetId) {
    if !asset_ids.iter().any(|existing| existing == &candidate) {
        asset_ids.push(candidate);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

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
        let _guard = ENV_LOCK.lock().unwrap();
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
    fn legacy_polymarket_env_names_populate_live_credentials() {
        let _guard = ENV_LOCK.lock().unwrap();
        let config_toml = r#"
[credentials]

[connection]

[websocket]
asset_ids = ["legacy-asset"]

[strategy]
strategy = "threshold"
token_id = "legacy-asset"
side = "Buy"
size = "1"
order_type = "FOK"

[strategy.params]
threshold = 0.5

[execution]

[logging]
"#;

        std::env::set_var("POLY_SECRET", "legacy-secret");
        std::env::set_var("POLY_ADDRESS", "0xlegacy-signer");
        std::env::set_var("POLY_PROXY_ADDRESS", "0xlegacy-maker");

        let mut config: ExecutorConfig = toml::from_str(config_toml).unwrap();
        config.apply_env_overrides();

        assert_eq!(config.credentials.api_secret, "legacy-secret");
        assert_eq!(config.credentials.signer_address, "0xlegacy-signer");
        assert_eq!(config.credentials.maker_address, "0xlegacy-maker");

        std::env::remove_var("POLY_SECRET");
        std::env::remove_var("POLY_ADDRESS");
        std::env::remove_var("POLY_PROXY_ADDRESS");
    }

    #[test]
    fn rtt_env_overrides_can_switch_checked_in_config_to_live_quote_mode() {
        let _guard = ENV_LOCK.lock().unwrap();
        let config_toml = r#"
[credentials]

[connection]

[websocket]
asset_ids = ["legacy-asset"]

[strategy]
strategy = "threshold"
token_id = "legacy-asset"
side = "Buy"
size = "1"
order_type = "FOK"

[strategy.params]
threshold = 0.5

[execution]
dry_run = true

[logging]
"#;

        std::env::set_var("RTT_STRATEGY", "liquidity_rewards");
        std::env::set_var("RTT_DRY_RUN", "false");
        std::env::set_var("RTT_MAX_TOTAL_DEPLOYED_USD", "100");
        std::env::set_var("RTT_BASE_QUOTE_SIZE", "50");
        std::env::set_var("RTT_ANALYSIS_DB_PATH", "/var/lib/rtt/analysis.sqlite");
        std::env::set_var("RTT_LOG_LEVEL", "debug");

        let mut config: ExecutorConfig = toml::from_str(config_toml).unwrap();
        config.apply_env_overrides();

        assert_eq!(config.strategy.strategy, "liquidity_rewards");
        assert!(!config.execution.dry_run);
        assert_eq!(config.strategy.params.max_total_deployed_usd, Some(100.0));
        assert_eq!(config.strategy.params.base_quote_size, Some(50.0));
        assert_eq!(
            config.quote_mode.analysis_db_path,
            "/var/lib/rtt/analysis.sqlite"
        );
        assert_eq!(config.logging.level, "debug");

        std::env::remove_var("RTT_STRATEGY");
        std::env::remove_var("RTT_DRY_RUN");
        std::env::remove_var("RTT_MAX_TOTAL_DEPLOYED_USD");
        std::env::remove_var("RTT_BASE_QUOTE_SIZE");
        std::env::remove_var("RTT_ANALYSIS_DB_PATH");
        std::env::remove_var("RTT_LOG_LEVEL");
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
    fn quote_mode_defaults_apply_without_section() {
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
        assert_eq!(config.quote_mode.analysis_db_path, "analysis.sqlite");
        assert_eq!(config.quote_mode.heartbeat_interval_secs, 5);
        assert_eq!(config.quote_mode.reward_poll_interval_secs, 60);
        assert_eq!(config.quote_mode.rebate_poll_interval_secs, 300);
    }

    #[test]
    fn quote_mode_section_parses_custom_values() {
        let config_toml = r#"
[credentials]
[connection]
[websocket]
asset_ids = ["asset1"]
[strategy]
strategy = "liquidity_rewards"
[strategy.params]
initial_bankroll_usd = 100
max_total_deployed_usd = 100
base_quote_size = 50
edge_buffer = 0.02
[execution]
[quote_mode]
analysis_db_path = "tmp/liquidity.sqlite"
clob_base_url = "https://clob-staging.polymarket.com"
user_ws_url = "wss://staging/ws/user"
heartbeat_interval_secs = 7
reward_poll_interval_secs = 90
rebate_poll_interval_secs = 600
[logging]
"#;

        let config: ExecutorConfig = toml::from_str(config_toml).unwrap();
        assert_eq!(config.quote_mode.analysis_db_path, "tmp/liquidity.sqlite");
        assert_eq!(
            config.quote_mode.clob_base_url,
            "https://clob-staging.polymarket.com"
        );
        assert_eq!(config.quote_mode.user_ws_url, "wss://staging/ws/user");
        assert_eq!(config.quote_mode.heartbeat_interval_secs, 7);
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
