use crate::liquidity_rewards::{LiquidityRewardsMarket, LiquidityRewardsParams, LiquidityRewardsStrategy};
use crate::spread::SpreadStrategy;
use crate::strategy::{QuoteStrategy, Strategy, TriggerStrategy};
use crate::threshold::ThresholdStrategy;
use crate::types::*;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Strategy-specific parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategyParams {
    /// Price threshold for ThresholdStrategy.
    pub threshold: Option<f64>,
    /// Maximum spread for SpreadStrategy.
    pub max_spread: Option<f64>,
    pub initial_bankroll_usd: Option<f64>,
    pub max_total_deployed_usd: Option<f64>,
    pub max_markets: Option<usize>,
    pub base_quote_size: Option<f64>,
    pub edge_buffer: Option<f64>,
    pub target_spread_cents: Option<f64>,
    pub quote_ttl_secs: Option<u64>,
    pub min_total_daily_rate: Option<f64>,
    pub max_market_competitiveness: Option<f64>,
    pub min_time_to_expiry_secs: Option<u64>,
    pub max_inventory_per_market: Option<f64>,
    pub max_unhedged_notional_per_market: Option<f64>,
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

fn default_side() -> Side {
    Side::Buy
}

fn default_size() -> String {
    "0".to_string()
}

fn default_order_type() -> OrderType {
    OrderType::GTC
}

impl StrategyConfig {
    /// Load config from a TOML file.
    pub fn from_file(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let content = std::fs::read_to_string(path)?;
        let config: Self = toml::from_str(&content)?;
        Ok(config)
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
            "liquidity_rewards" => Err("liquidity_rewards is a quote strategy".to_string()),
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
            "liquidity_rewards" => Err("liquidity_rewards is a quote strategy".to_string()),
            other => Err(format!("unknown strategy: {}", other)),
        }
    }

    pub fn build_quote_strategy(
        &self,
        markets: Vec<LiquidityRewardsMarket>,
    ) -> Result<Box<dyn QuoteStrategy>, String> {
        match self.strategy.as_str() {
            "liquidity_rewards" => Ok(Box::new(LiquidityRewardsStrategy::new(
                markets,
                self.liquidity_rewards_params(),
            ))),
            other => Err(format!("{other} is not a quote strategy")),
        }
    }

    pub fn liquidity_rewards_params(&self) -> LiquidityRewardsParams {
        LiquidityRewardsParams {
            initial_bankroll_usd: self.params.initial_bankroll_usd.unwrap_or(100.0),
            max_total_deployed_usd: self.params.max_total_deployed_usd.unwrap_or(100.0),
            max_markets: self.params.max_markets.unwrap_or(2),
            base_quote_size: self.params.base_quote_size.unwrap_or(50.0),
            edge_buffer: self.params.edge_buffer.unwrap_or(0.02),
            target_spread_cents: self.params.target_spread_cents.unwrap_or(2.0),
            quote_ttl_secs: self.params.quote_ttl_secs.unwrap_or(30),
            min_total_daily_rate: self.params.min_total_daily_rate.unwrap_or(1.0),
            max_market_competitiveness: self.params.max_market_competitiveness.unwrap_or(10.0),
            min_time_to_expiry_secs: self.params.min_time_to_expiry_secs.unwrap_or(300),
            max_inventory_per_market: self.params.max_inventory_per_market.unwrap_or(100.0),
            max_unhedged_notional_per_market: self
                .params
                .max_unhedged_notional_per_market
                .unwrap_or(40.0),
        }
    }
}
