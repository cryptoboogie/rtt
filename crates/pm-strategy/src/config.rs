use crate::spread::SpreadStrategy;
use crate::strategy::{Strategy, TriggerStrategy};
use crate::threshold::ThresholdStrategy;
use crate::types::*;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Btc5mRiskMode {
    Historical,
    #[default]
    SmallAccount,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Btc5mParams {
    #[serde(default = "default_market_slug_prefix")]
    pub market_slug_prefix: String,
    #[serde(default = "default_cadence_seconds")]
    pub cadence_seconds: u64,
    #[serde(default = "default_prefetch_markets")]
    pub prefetch_markets: usize,
    #[serde(default = "default_entry_window_start_seconds")]
    pub entry_window_start_seconds: u64,
    #[serde(default = "default_entry_window_end_seconds")]
    pub entry_window_end_seconds: u64,
    #[serde(default = "default_probe_window_end_seconds")]
    pub probe_window_end_seconds: u64,
    #[serde(default = "default_probe_budget_usd")]
    pub probe_budget_usd: f64,
    #[serde(default = "default_initial_burst_budget_usd")]
    pub initial_burst_budget_usd: f64,
    #[serde(default = "default_max_pair_budget_usd")]
    pub max_pair_budget_usd: f64,
    #[serde(default = "default_max_single_side_budget_usd")]
    pub max_single_side_budget_usd: f64,
    #[serde(default = "default_max_gross_deployed_per_market")]
    pub max_gross_deployed_per_market: f64,
    #[serde(default = "default_max_unpaired_exposure_usd")]
    pub max_unpaired_exposure_usd: f64,
    #[serde(default = "default_max_cleanup_loss_usd")]
    pub max_cleanup_loss_usd: f64,
    #[serde(default = "default_carry_pair_sum_max")]
    pub carry_pair_sum_max: f64,
    #[serde(default = "default_attempt_cooldown_ms")]
    pub attempt_cooldown_ms: u64,
    #[serde(default = "default_cleanup_grace_ms")]
    pub cleanup_grace_ms: u64,
    #[serde(default = "default_binance_ws_url")]
    pub binance_ws_url: String,
    #[serde(default = "default_binance_stale_after_ms")]
    pub binance_stale_after_ms: u64,
    #[serde(default = "default_binance_buffer_window_ms")]
    pub binance_buffer_window_ms: u64,
    #[serde(default = "default_binance_continuation_window_ms")]
    pub binance_continuation_window_ms: u64,
    #[serde(default = "default_binance_min_move_bps")]
    pub binance_min_move_bps: f64,
    #[serde(default = "default_binance_reversal_veto_bps")]
    pub binance_reversal_veto_bps: f64,
    #[serde(default)]
    pub allow_one_sided_continuation: bool,
    #[serde(default = "default_one_sided_min_aligned_entry_bps")]
    pub one_sided_min_aligned_entry_bps: f64,
    #[serde(default)]
    pub risk_mode: Btc5mRiskMode,
}

impl Default for Btc5mParams {
    fn default() -> Self {
        Self {
            market_slug_prefix: default_market_slug_prefix(),
            cadence_seconds: default_cadence_seconds(),
            prefetch_markets: default_prefetch_markets(),
            entry_window_start_seconds: default_entry_window_start_seconds(),
            entry_window_end_seconds: default_entry_window_end_seconds(),
            probe_window_end_seconds: default_probe_window_end_seconds(),
            probe_budget_usd: default_probe_budget_usd(),
            initial_burst_budget_usd: default_initial_burst_budget_usd(),
            max_pair_budget_usd: default_max_pair_budget_usd(),
            max_single_side_budget_usd: default_max_single_side_budget_usd(),
            max_gross_deployed_per_market: default_max_gross_deployed_per_market(),
            max_unpaired_exposure_usd: default_max_unpaired_exposure_usd(),
            max_cleanup_loss_usd: default_max_cleanup_loss_usd(),
            carry_pair_sum_max: default_carry_pair_sum_max(),
            attempt_cooldown_ms: default_attempt_cooldown_ms(),
            cleanup_grace_ms: default_cleanup_grace_ms(),
            binance_ws_url: default_binance_ws_url(),
            binance_stale_after_ms: default_binance_stale_after_ms(),
            binance_buffer_window_ms: default_binance_buffer_window_ms(),
            binance_continuation_window_ms: default_binance_continuation_window_ms(),
            binance_min_move_bps: default_binance_min_move_bps(),
            binance_reversal_veto_bps: default_binance_reversal_veto_bps(),
            allow_one_sided_continuation: false,
            one_sided_min_aligned_entry_bps: default_one_sided_min_aligned_entry_bps(),
            risk_mode: Btc5mRiskMode::default(),
        }
    }
}

/// Strategy-specific parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategyParams {
    /// Price threshold for ThresholdStrategy.
    pub threshold: Option<f64>,
    /// Maximum spread for SpreadStrategy.
    pub max_spread: Option<f64>,
    /// Parameters for the dedicated BTC 5m runtime.
    #[serde(default)]
    pub btc_5m: Option<Btc5mParams>,
}

/// Top-level strategy configuration, loadable from TOML.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategyConfig {
    /// Strategy name: "threshold" or "spread".
    pub strategy: String,
    /// Target asset/token ID.
    #[serde(default)]
    pub token_id: String,
    /// Order side.
    #[serde(default = "default_side")]
    pub side: Side,
    /// Order size as decimal string.
    #[serde(default = "default_size")]
    pub size: String,
    /// Order type.
    #[serde(default = "default_order_type")]
    pub order_type: OrderType,
    /// Strategy-specific parameters.
    pub params: StrategyParams,
}

impl StrategyConfig {
    /// Load config from a TOML file.
    pub fn from_file(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let content = std::fs::read_to_string(path)?;
        let config: Self = toml::from_str(&content)?;
        Ok(config)
    }

    pub fn uses_specialized_runtime(&self) -> bool {
        self.strategy == "btc_5m"
    }

    pub fn btc_5m_params(&self) -> Result<Btc5mParams, String> {
        if !self.uses_specialized_runtime() {
            return Err(format!(
                "strategy {} does not use the btc_5m runtime",
                self.strategy
            ));
        }

        Ok(self.params.btc_5m.clone().unwrap_or_default())
    }

    /// Build the concrete strategy from this config.
    pub fn build_strategy(&self) -> Result<Box<dyn Strategy>, String> {
        match self.strategy.as_str() {
            "threshold" => {
                let threshold = self
                    .params
                    .threshold
                    .ok_or("threshold strategy requires 'threshold' param")?;
                Ok(Box::new(ThresholdStrategy::new(
                    self.token_id.clone(),
                    self.side,
                    threshold,
                    self.size.clone(),
                    self.order_type,
                )))
            }
            "spread" => {
                let max_spread = self
                    .params
                    .max_spread
                    .ok_or("spread strategy requires 'max_spread' param")?;
                Ok(Box::new(SpreadStrategy::new(
                    self.token_id.clone(),
                    self.side,
                    max_spread,
                    self.size.clone(),
                    self.order_type,
                )))
            }
            "btc_5m" => Err("btc_5m uses the dedicated executor runtime".to_string()),
            other => Err(format!("unknown strategy: {}", other)),
        }
    }

    /// Build the concrete trigger-strategy contract from this config.
    pub fn build_trigger_strategy(&self) -> Result<Box<dyn TriggerStrategy>, String> {
        match self.strategy.as_str() {
            "threshold" => {
                let threshold = self
                    .params
                    .threshold
                    .ok_or("threshold strategy requires 'threshold' param")?;
                Ok(Box::new(ThresholdStrategy::new(
                    self.token_id.clone(),
                    self.side,
                    threshold,
                    self.size.clone(),
                    self.order_type,
                )))
            }
            "spread" => {
                let max_spread = self
                    .params
                    .max_spread
                    .ok_or("spread strategy requires 'max_spread' param")?;
                Ok(Box::new(SpreadStrategy::new(
                    self.token_id.clone(),
                    self.side,
                    max_spread,
                    self.size.clone(),
                    self.order_type,
                )))
            }
            "btc_5m" => Err("btc_5m uses the dedicated executor runtime".to_string()),
            other => Err(format!("unknown strategy: {}", other)),
        }
    }
}

fn default_market_slug_prefix() -> String {
    "btc-updown-5m".to_string()
}

fn default_cadence_seconds() -> u64 {
    300
}

fn default_prefetch_markets() -> usize {
    1
}

fn default_entry_window_start_seconds() -> u64 {
    5
}

fn default_entry_window_end_seconds() -> u64 {
    21
}

fn default_probe_window_end_seconds() -> u64 {
    9
}

fn default_probe_budget_usd() -> f64 {
    3.0
}

fn default_initial_burst_budget_usd() -> f64 {
    5.0
}

fn default_max_pair_budget_usd() -> f64 {
    45.0
}

fn default_max_single_side_budget_usd() -> f64 {
    10.0
}

fn default_max_gross_deployed_per_market() -> f64 {
    50.0
}

fn default_max_unpaired_exposure_usd() -> f64 {
    12.0
}

fn default_max_cleanup_loss_usd() -> f64 {
    5.0
}

fn default_carry_pair_sum_max() -> f64 {
    0.96
}

fn default_attempt_cooldown_ms() -> u64 {
    1_000
}

fn default_cleanup_grace_ms() -> u64 {
    1_500
}

fn default_binance_ws_url() -> String {
    "wss://stream.binance.com:9443/ws/btcusdt@aggTrade".to_string()
}

fn default_binance_stale_after_ms() -> u64 {
    1_500
}

fn default_binance_buffer_window_ms() -> u64 {
    5_000
}

fn default_binance_continuation_window_ms() -> u64 {
    3_000
}

fn default_binance_min_move_bps() -> f64 {
    0.5
}

fn default_binance_reversal_veto_bps() -> f64 {
    0.5
}

fn default_one_sided_min_aligned_entry_bps() -> f64 {
    2.0
}

fn default_side() -> Side {
    Side::Buy
}

fn default_size() -> String {
    "5".to_string()
}

fn default_order_type() -> OrderType {
    OrderType::FOK
}
