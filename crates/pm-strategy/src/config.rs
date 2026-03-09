use crate::spread::SpreadStrategy;
use crate::strategy::{Strategy, TriggerStrategy};
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
}

/// Top-level strategy configuration, loadable from TOML.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategyConfig {
    /// Strategy name: "threshold" or "spread".
    pub strategy: String,
    /// Target asset/token ID.
    pub token_id: String,
    /// Order side.
    pub side: Side,
    /// Order size as decimal string.
    pub size: String,
    /// Order type.
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
            other => Err(format!("unknown strategy: {}", other)),
        }
    }
}
