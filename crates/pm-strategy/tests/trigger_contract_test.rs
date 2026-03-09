use pm_strategy::strategy::TriggerStrategy;
use pm_strategy::strategy::{ExecutionMode, IsolationPolicy, StrategyDataRequirement};
use pm_strategy::threshold::ThresholdStrategy;
use pm_strategy::{OrderType, Side};

#[test]
fn threshold_strategy_declares_trigger_requirements() {
    let strategy = ThresholdStrategy::new(
        "token_abc".to_string(),
        Side::Buy,
        0.45,
        "50".to_string(),
        OrderType::FOK,
    );

    let requirements = TriggerStrategy::requirements(&strategy);

    assert_eq!(requirements.execution_mode, ExecutionMode::Trigger);
    assert_eq!(
        requirements.isolation_policy,
        IsolationPolicy::SharedFeedAcceptable
    );
    assert_eq!(
        requirements.data,
        vec![StrategyDataRequirement::polymarket_bbo("token_abc")]
    );
}
