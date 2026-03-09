use std::collections::{BTreeMap, BTreeSet};

use rtt_core::{MarketId, MarketMeta};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistrySnapshot {
    pub markets: BTreeMap<MarketId, MarketMeta>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct UniverseSelectionPolicy {
    pub active_only: bool,
    pub require_reward: bool,
    pub include_markets: BTreeSet<MarketId>,
    pub exclude_markets: BTreeSet<MarketId>,
    pub bypass_registry: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectedUniverse {
    pub selected_market_ids: Vec<MarketId>,
    pub decisions: Vec<UniverseDecision>,
    pub bypassed: bool,
    pub bypass_reason: Option<UniverseBypassReason>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UniverseDecision {
    pub market_id: MarketId,
    pub decision: UniverseDecisionKind,
    pub reason: UniverseSelectionReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UniverseDecisionKind {
    Included,
    Excluded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UniverseSelectionReason {
    Active,
    ExplicitInclude,
    ExplicitExclude,
    InactiveStatus,
    MissingReward,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UniverseBypassReason {
    ExplicitSourceBindings,
}

impl SelectedUniverse {
    pub fn resolve(
        snapshot: Option<&RegistrySnapshot>,
        policy: &UniverseSelectionPolicy,
    ) -> Self {
        if policy.bypass_registry {
            return Self {
                selected_market_ids: Vec::new(),
                decisions: Vec::new(),
                bypassed: true,
                bypass_reason: Some(UniverseBypassReason::ExplicitSourceBindings),
            };
        }

        let Some(snapshot) = snapshot else {
            return Self {
                selected_market_ids: Vec::new(),
                decisions: Vec::new(),
                bypassed: false,
                bypass_reason: None,
            };
        };

        let mut selected_market_ids = Vec::new();
        let mut decisions = Vec::with_capacity(snapshot.markets.len());
        for (market_id, market) in &snapshot.markets {
            let (decision, reason) = if policy.exclude_markets.contains(market_id) {
                (UniverseDecisionKind::Excluded, UniverseSelectionReason::ExplicitExclude)
            } else if policy.include_markets.contains(market_id) {
                (UniverseDecisionKind::Included, UniverseSelectionReason::ExplicitInclude)
            } else if policy.active_only && !market.is_tradable() {
                (UniverseDecisionKind::Excluded, UniverseSelectionReason::InactiveStatus)
            } else if policy.require_reward && market.reward.is_none() {
                (UniverseDecisionKind::Excluded, UniverseSelectionReason::MissingReward)
            } else {
                (UniverseDecisionKind::Included, UniverseSelectionReason::Active)
            };

            if decision == UniverseDecisionKind::Included {
                selected_market_ids.push(market_id.clone());
            }

            decisions.push(UniverseDecision {
                market_id: market_id.clone(),
                decision,
                reason,
            });
        }

        Self {
            selected_market_ids,
            decisions,
            bypassed: false,
            bypass_reason: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rtt_core::{
        AssetId, MarketStatus, MinOrderSize, OutcomeSide, OutcomeToken, RewardFreshness,
        RewardParams, TickSize,
    };

    fn market(
        market_id: &str,
        status: MarketStatus,
        reward: bool,
    ) -> (MarketId, MarketMeta) {
        let market_id = MarketId::new(market_id);
        (
            market_id.clone(),
            MarketMeta {
                market_id: market_id.clone(),
                yes_asset: OutcomeToken::new(
                    AssetId::new(format!("{market_id}-yes")),
                    OutcomeSide::Yes,
                ),
                no_asset: OutcomeToken::new(
                    AssetId::new(format!("{market_id}-no")),
                    OutcomeSide::No,
                ),
                condition_id: Some(format!("condition-{market_id}")),
                tick_size: TickSize::new("0.01"),
                min_order_size: Some(MinOrderSize::new("5")),
                status,
                reward: reward.then_some(RewardParams {
                    rate_bps: Some(12),
                    max_spread: None,
                    min_size: None,
                    min_notional: None,
                    updated_at_ms: Some(1_700_000_000_000),
                    freshness: RewardFreshness::Fresh,
                }),
            },
        )
    }

    #[test]
    fn selection_policy_produces_deterministic_reasons() {
        let markets = BTreeMap::from([
            market("market-1", MarketStatus::Active, true),
            market("market-2", MarketStatus::Active, false),
            market("market-3", MarketStatus::Closed, true),
            market("market-4", MarketStatus::Active, false),
        ]);
        let snapshot = RegistrySnapshot { markets };
        let policy = UniverseSelectionPolicy {
            active_only: true,
            require_reward: true,
            include_markets: BTreeSet::from([MarketId::new("market-2")]),
            exclude_markets: BTreeSet::from([MarketId::new("market-1")]),
            bypass_registry: false,
        };

        let universe = SelectedUniverse::resolve(Some(&snapshot), &policy);

        let json = serde_json::to_value(&universe).unwrap();
        assert_eq!(json["selected_market_ids"], serde_json::json!(["market-2"]));
        assert_eq!(
            json["decisions"][0],
            serde_json::json!({
                "market_id": "market-1",
                "decision": "excluded",
                "reason": "explicit_exclude",
            })
        );
        assert_eq!(
            json["decisions"][1],
            serde_json::json!({
                "market_id": "market-2",
                "decision": "included",
                "reason": "explicit_include",
            })
        );
        assert_eq!(
            json["decisions"][2],
            serde_json::json!({
                "market_id": "market-3",
                "decision": "excluded",
                "reason": "inactive_status",
            })
        );
        assert_eq!(
            json["decisions"][3],
            serde_json::json!({
                "market_id": "market-4",
                "decision": "excluded",
                "reason": "missing_reward",
            })
        );
    }

    #[test]
    fn explicit_bindings_can_bypass_registry_selection() {
        let policy = UniverseSelectionPolicy {
            bypass_registry: true,
            ..UniverseSelectionPolicy::default()
        };

        let universe = SelectedUniverse::resolve(None, &policy);
        let json = serde_json::to_value(&universe).unwrap();

        assert_eq!(json["selected_market_ids"], serde_json::json!([]));
        assert_eq!(json["decisions"], serde_json::json!([]));
        assert_eq!(json["bypassed"], serde_json::json!(true));
        assert_eq!(
            json["bypass_reason"],
            serde_json::json!("explicit_source_bindings")
        );
    }
}
