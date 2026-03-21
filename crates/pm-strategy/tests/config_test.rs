use pm_strategy::config::StrategyConfig;
use pm_strategy::liquidity_rewards::LiquidityRewardsMarket;
use pm_strategy::*;

#[test]
fn parse_threshold_config_from_toml() {
    let toml_str = r#"
strategy = "threshold"
token_id = "0xabc123"
side = "Buy"
size = "50"
order_type = "FOK"

[params]
threshold = 0.40
"#;

    let config: StrategyConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.strategy, "threshold");
    assert_eq!(config.token_id, "0xabc123");
    assert_eq!(config.side, Side::Buy);
    assert_eq!(config.size, "50");
    assert_eq!(config.order_type, OrderType::FOK);
    assert_eq!(config.params.threshold, Some(0.40));
    assert!(config.params.max_spread.is_none());
}

#[test]
fn parse_spread_config_from_toml() {
    let toml_str = r#"
strategy = "spread"
token_id = "token_xyz"
side = "Sell"
size = "25"
order_type = "GTC"

[params]
max_spread = 0.02
"#;

    let config: StrategyConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.strategy, "spread");
    assert_eq!(config.token_id, "token_xyz");
    assert_eq!(config.side, Side::Sell);
    assert_eq!(config.size, "25");
    assert_eq!(config.order_type, OrderType::GTC);
    assert_eq!(config.params.max_spread, Some(0.02));
    assert!(config.params.threshold.is_none());
}

#[test]
fn config_builds_threshold_strategy() {
    let toml_str = r#"
strategy = "threshold"
token_id = "token_abc"
side = "Buy"
size = "50"
order_type = "FOK"

[params]
threshold = 0.40
"#;

    let config: StrategyConfig = toml::from_str(toml_str).unwrap();
    let strat = config.build_strategy().unwrap();
    assert_eq!(strat.name(), "threshold");
}

#[test]
fn config_builds_spread_strategy() {
    let toml_str = r#"
strategy = "spread"
token_id = "token_abc"
side = "Sell"
size = "25"
order_type = "GTC"

[params]
max_spread = 0.02
"#;

    let config: StrategyConfig = toml::from_str(toml_str).unwrap();
    let strat = config.build_strategy().unwrap();
    assert_eq!(strat.name(), "spread");
}

#[test]
fn config_builds_threshold_trigger_strategy() {
    let toml_str = r#"
strategy = "threshold"
token_id = "token_abc"
side = "Buy"
size = "50"
order_type = "FOK"

[params]
threshold = 0.40
"#;

    let config: StrategyConfig = toml::from_str(toml_str).unwrap();
    let strat = config.build_trigger_strategy().unwrap();
    assert_eq!(strat.name(), "threshold");
    assert_eq!(
        strat.requirements().execution_mode,
        pm_strategy::strategy::ExecutionMode::Trigger
    );
}

#[test]
fn config_unknown_strategy_returns_error() {
    let toml_str = r#"
strategy = "unknown"
token_id = "token_abc"
side = "Buy"
size = "50"
order_type = "FOK"

[params]
"#;

    let config: StrategyConfig = toml::from_str(toml_str).unwrap();
    assert!(config.build_strategy().is_err());
}

#[test]
fn config_missing_threshold_param_returns_error() {
    let toml_str = r#"
strategy = "threshold"
token_id = "token_abc"
side = "Buy"
size = "50"
order_type = "FOK"

[params]
"#;

    let config: StrategyConfig = toml::from_str(toml_str).unwrap();
    assert!(config.build_strategy().is_err());
}

#[test]
fn config_roundtrip_toml_serialize() {
    let toml_str = r#"
strategy = "threshold"
token_id = "token_abc"
side = "Buy"
size = "50"
order_type = "FOK"

[params]
threshold = 0.4
"#;

    let config: StrategyConfig = toml::from_str(toml_str).unwrap();
    let serialized = toml::to_string(&config).unwrap();
    let reparsed: StrategyConfig = toml::from_str(&serialized).unwrap();
    assert_eq!(reparsed.strategy, config.strategy);
    assert_eq!(reparsed.token_id, config.token_id);
}

#[test]
fn config_load_from_file() {
    use std::io::Write;

    let dir = std::env::temp_dir().join("pm_strategy_test");
    std::fs::create_dir_all(&dir).ok();
    let path = dir.join("test_config.toml");

    let toml_str = r#"
strategy = "spread"
token_id = "token_xyz"
side = "Sell"
size = "25"
order_type = "GTC"

[params]
max_spread = 0.02
"#;

    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(toml_str.as_bytes()).unwrap();

    let config = StrategyConfig::from_file(&path).unwrap();
    assert_eq!(config.strategy, "spread");
    assert_eq!(config.params.max_spread, Some(0.02));

    std::fs::remove_file(&path).ok();
}

#[test]
fn parse_liquidity_rewards_config_from_toml() {
    let toml_str = r#"
strategy = "liquidity_rewards"

[params]
initial_bankroll_usd = 100
max_total_deployed_usd = 100
max_markets = 2
base_quote_size = 50
edge_buffer = 0.02
target_spread_cents = 2
quote_ttl_secs = 30
min_total_daily_rate = 1
max_market_competitiveness = 10
min_time_to_expiry_secs = 300
max_inventory_per_market = 100
max_unhedged_notional_per_market = 40
"#;

    let config: StrategyConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.strategy, "liquidity_rewards");
    assert_eq!(config.token_id, "");
    assert_eq!(config.order_type, OrderType::GTC);
    assert_eq!(config.params.max_markets, Some(2));
}

#[test]
fn config_builds_liquidity_rewards_quote_strategy() {
    let toml_str = r#"
strategy = "liquidity_rewards"

[params]
base_quote_size = 50
"#;

    let config: StrategyConfig = toml::from_str(toml_str).unwrap();
    let strategy = config
        .build_quote_strategy(vec![LiquidityRewardsMarket {
            condition_id: "condition-1".to_string(),
            yes_asset_id: "yes-asset".to_string(),
            no_asset_id: "no-asset".to_string(),
            neg_risk: false,
            tick_size: "0.01".to_string(),
            min_order_size: Some("5".to_string()),
            reward_max_spread: "0.04".to_string(),
            reward_min_size: "50".to_string(),
            end_time_ms: None,
        }])
        .unwrap();

    assert_eq!(strategy.name(), "liquidity_rewards");
    assert_eq!(
        strategy.requirements().execution_mode,
        pm_strategy::strategy::ExecutionMode::Quote
    );
}
