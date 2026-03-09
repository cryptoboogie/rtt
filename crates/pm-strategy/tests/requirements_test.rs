use pm_strategy::strategy::{
    ExecutionMode, IsolationPolicy, StrategyDataRequirement, StrategyRequirements,
};

#[test]
fn strategy_requirements_capture_mode_and_isolation_hints() {
    let requirements = StrategyRequirements::trigger(
        vec![
            StrategyDataRequirement::polymarket_bbo("token-abc"),
            StrategyDataRequirement::external_reference_price("BTC-USD"),
        ],
        IsolationPolicy::DedicatedPreferred,
    );

    assert_eq!(requirements.execution_mode, ExecutionMode::Trigger);
    assert_eq!(
        requirements.isolation_policy,
        IsolationPolicy::DedicatedPreferred
    );
    assert_eq!(requirements.data.len(), 2);
}
