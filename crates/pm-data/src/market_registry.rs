use std::time::Duration;
use std::{
    collections::BTreeMap,
    sync::RwLock,
    time::{SystemTime, UNIX_EPOCH},
};

use rtt_core::{MarketId, MarketMeta};
use serde::Serialize;

use crate::{
    registry_provider::{
        CurrentRewardConfig, RawRewardMarket, RegistryPageRequest, RegistryProvider,
        RegistryProviderError,
    },
    snapshot::{RegistrySnapshot, SelectedUniverse, UniverseSelectionPolicy},
};

pub struct MarketRegistry<P> {
    provider: P,
    refresh_policy: RegistryRefreshPolicy,
    selection_policy: UniverseSelectionPolicy,
    state: RwLock<RegistryState>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryRefreshPolicy {
    pub page_size: usize,
    pub refresh_interval: Duration,
    pub retry_policy: RetryPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetryPolicy {
    pub max_retries: usize,
    pub initial_backoff: Duration,
    pub max_backoff: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RegistryRefreshOutcome {
    pub snapshot: RegistrySnapshot,
    pub universe: SelectedUniverse,
    pub degraded: bool,
    pub attempts: usize,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryRefreshError {
    pub message: String,
}

#[derive(Debug, Default)]
struct RegistryState {
    current_snapshot: Option<RegistrySnapshot>,
    current_universe: Option<SelectedUniverse>,
    last_good_snapshot: Option<RegistrySnapshot>,
    last_good_universe: Option<SelectedUniverse>,
    next_sequence: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RewardDiscoveryMarket {
    pub market: MarketMeta,
    pub end_time_ms: Option<u64>,
    pub accepting_orders: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RewardSelectionPolicy {
    pub max_markets: usize,
    pub max_total_deployed_usd: f64,
    pub base_quote_size: f64,
    pub edge_buffer: f64,
    pub min_total_daily_rate: f64,
    pub max_market_competitiveness: f64,
    pub min_time_to_expiry_secs: u64,
    pub max_reward_age_ms: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SelectedRewardMarket {
    pub market: MarketMeta,
    pub end_time_ms: Option<u64>,
    pub reserved_capital_usd: f64,
    pub reward_per_reserved_usd: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RewardMarketSelection {
    pub selected: Vec<SelectedRewardMarket>,
    pub decisions: Vec<RewardSelectionDecision>,
    pub total_reserved_capital_usd: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RewardSelectionDecision {
    pub market_id: MarketId,
    pub reason: RewardSelectionReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RewardSelectionReason {
    Selected,
    InactiveStatus,
    AcceptingOrdersDisabled,
    MissingReward,
    RewardStale,
    NearExpiry,
    UnderRewarded,
    OverCompetitive,
    InsufficientSpreadBudget,
    DeploymentBudgetExceeded,
}

impl RetryPolicy {
    fn backoff_for_retry(&self, retry_index: usize) -> Duration {
        let multiplier = 1u32.checked_shl(retry_index as u32).unwrap_or(u32::MAX);
        let delay = self.initial_backoff.saturating_mul(multiplier);
        delay.min(self.max_backoff)
    }
}

impl RegistryRefreshPolicy {
    pub fn is_refresh_due(&self, last_refresh_ms: Option<u64>, now_ms: u64) -> bool {
        match last_refresh_ms {
            None => true,
            Some(last_refresh_ms) => {
                now_ms.saturating_sub(last_refresh_ms) >= self.refresh_interval.as_millis() as u64
            }
        }
    }
}

pub fn select_reward_markets(
    markets: &[RewardDiscoveryMarket],
    policy: &RewardSelectionPolicy,
    now_ms: u64,
) -> RewardMarketSelection {
    let mut decisions = Vec::with_capacity(markets.len());
    let mut ranked = Vec::new();

    for candidate in markets {
        let Some(reward) = candidate.market.reward.as_ref() else {
            decisions.push(RewardSelectionDecision {
                market_id: candidate.market.market_id.clone(),
                reason: RewardSelectionReason::MissingReward,
            });
            continue;
        };

        let reason = if !candidate.market.is_tradable() {
            Some(RewardSelectionReason::InactiveStatus)
        } else if !candidate.accepting_orders {
            Some(RewardSelectionReason::AcceptingOrdersDisabled)
        } else if reward.freshness != rtt_core::RewardFreshness::Fresh
            || reward
                .updated_at_ms
                .map(|updated_at_ms| {
                    now_ms.saturating_sub(updated_at_ms) > policy.max_reward_age_ms
                })
                .unwrap_or(true)
        {
            Some(RewardSelectionReason::RewardStale)
        } else if candidate
            .end_time_ms
            .map(|end_time_ms| {
                end_time_ms.saturating_sub(now_ms)
                    < policy.min_time_to_expiry_secs.saturating_mul(1_000)
            })
            .unwrap_or(false)
        {
            Some(RewardSelectionReason::NearExpiry)
        } else if reward_total_daily_rate(reward) < policy.min_total_daily_rate {
            Some(RewardSelectionReason::UnderRewarded)
        } else if reward_competitiveness(reward) > policy.max_market_competitiveness {
            Some(RewardSelectionReason::OverCompetitive)
        } else if reward_max_spread(reward) * 2.0 < policy.edge_buffer {
            Some(RewardSelectionReason::InsufficientSpreadBudget)
        } else {
            None
        };

        if let Some(reason) = reason {
            decisions.push(RewardSelectionDecision {
                market_id: candidate.market.market_id.clone(),
                reason,
            });
            continue;
        }

        let reserved_capital_usd = estimated_reserved_capital_usd(candidate, policy);
        if reserved_capital_usd > policy.max_total_deployed_usd {
            decisions.push(RewardSelectionDecision {
                market_id: candidate.market.market_id.clone(),
                reason: RewardSelectionReason::DeploymentBudgetExceeded,
            });
            continue;
        }

        ranked.push((
            candidate.market.market_id.clone(),
            SelectedRewardMarket {
                market: candidate.market.clone(),
                end_time_ms: candidate.end_time_ms,
                reserved_capital_usd,
                reward_per_reserved_usd: reward_total_daily_rate(reward) / reserved_capital_usd,
            },
        ));
    }

    ranked.sort_by(|(_, left), (_, right)| {
        right
            .reward_per_reserved_usd
            .partial_cmp(&left.reward_per_reserved_usd)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                left.market
                    .market_id
                    .as_str()
                    .cmp(right.market.market_id.as_str())
            })
    });

    let mut selected = Vec::new();
    let mut remaining_budget = policy.max_total_deployed_usd;
    for (market_id, candidate) in ranked {
        if selected.len() >= policy.max_markets {
            break;
        }
        if candidate.reserved_capital_usd > remaining_budget {
            decisions.push(RewardSelectionDecision {
                market_id,
                reason: RewardSelectionReason::DeploymentBudgetExceeded,
            });
            continue;
        }

        remaining_budget -= candidate.reserved_capital_usd;
        decisions.push(RewardSelectionDecision {
            market_id,
            reason: RewardSelectionReason::Selected,
        });
        selected.push(candidate);
    }

    RewardMarketSelection {
        total_reserved_capital_usd: selected
            .iter()
            .map(|market| market.reserved_capital_usd)
            .sum(),
        selected,
        decisions,
    }
}

pub fn enrich_reward_markets(
    markets: &[RewardDiscoveryMarket],
    current_configs: &[CurrentRewardConfig],
    raw_rewards: &[RawRewardMarket],
) -> Vec<RewardDiscoveryMarket> {
    let current_by_condition: BTreeMap<&str, &CurrentRewardConfig> = current_configs
        .iter()
        .map(|config| (config.condition_id.as_str(), config))
        .collect();
    let raw_by_condition: BTreeMap<&str, &RawRewardMarket> = raw_rewards
        .iter()
        .map(|market| (market.condition_id.as_str(), market))
        .collect();

    markets
        .iter()
        .cloned()
        .map(|mut discovery| {
            let Some(condition_id) = discovery.market.condition_id.as_deref() else {
                return discovery;
            };
            let Some(current) = current_by_condition.get(condition_id) else {
                return discovery;
            };

            let raw = raw_by_condition.get(condition_id).copied();
            let existing_fee_enabled = discovery
                .market
                .reward
                .as_ref()
                .and_then(|reward| reward.fee_enabled);

            let mut reward = current.reward.clone();
            reward.market_competitiveness = raw.and_then(|row| row.market_competitiveness.clone());
            reward.fee_enabled = existing_fee_enabled;
            discovery.market.reward = Some(reward);
            discovery
        })
        .collect()
}

fn reward_total_daily_rate(reward: &rtt_core::RewardParams) -> f64 {
    reward
        .total_daily_rate
        .as_ref()
        .and_then(|value| value.as_str().parse::<f64>().ok())
        .unwrap_or(0.0)
}

fn reward_competitiveness(reward: &rtt_core::RewardParams) -> f64 {
    reward
        .market_competitiveness
        .as_deref()
        .and_then(|value| value.parse::<f64>().ok())
        .unwrap_or(f64::INFINITY)
}

fn reward_max_spread(reward: &rtt_core::RewardParams) -> f64 {
    reward
        .max_spread
        .as_ref()
        .and_then(|value| value.as_str().parse::<f64>().ok())
        .unwrap_or(0.0)
}

fn estimated_reserved_capital_usd(
    candidate: &RewardDiscoveryMarket,
    policy: &RewardSelectionPolicy,
) -> f64 {
    let reward_min_size = candidate
        .market
        .reward
        .as_ref()
        .and_then(|reward| reward.min_size.as_ref())
        .and_then(|value| value.as_str().parse::<f64>().ok())
        .unwrap_or(0.0);
    let market_min_size = candidate
        .market
        .min_order_size
        .as_ref()
        .and_then(|value| value.as_str().parse::<f64>().ok())
        .unwrap_or(0.0);
    let quote_size = policy
        .base_quote_size
        .max(reward_min_size)
        .max(market_min_size);
    quote_size * (1.0 - policy.edge_buffer)
}

impl<P: RegistryProvider> MarketRegistry<P> {
    pub fn new(
        provider: P,
        refresh_policy: RegistryRefreshPolicy,
        selection_policy: UniverseSelectionPolicy,
    ) -> Self {
        Self {
            provider,
            refresh_policy,
            selection_policy,
            state: RwLock::new(RegistryState::default()),
        }
    }

    pub async fn refresh_once(&self) -> Result<RegistryRefreshOutcome, RegistryRefreshError> {
        match self.collect_snapshot().await {
            Ok((snapshot, attempts)) => {
                let universe = SelectedUniverse::resolve(Some(&snapshot), &self.selection_policy);
                let mut state = self.state.write().unwrap();
                state.current_snapshot = Some(snapshot.clone());
                state.current_universe = Some(universe.clone());
                state.last_good_snapshot = Some(snapshot.clone());
                state.last_good_universe = Some(universe.clone());

                Ok(RegistryRefreshOutcome {
                    snapshot,
                    universe,
                    degraded: false,
                    attempts,
                    error: None,
                })
            }
            Err((err, attempts)) => {
                let state = self.state.read().unwrap();
                match (&state.last_good_snapshot, &state.last_good_universe) {
                    (Some(snapshot), Some(universe)) => Ok(RegistryRefreshOutcome {
                        snapshot: snapshot.clone(),
                        universe: universe.clone(),
                        degraded: true,
                        attempts,
                        error: Some(err.to_string()),
                    }),
                    _ => Err(RegistryRefreshError {
                        message: err.to_string(),
                    }),
                }
            }
        }
    }

    async fn collect_snapshot(
        &self,
    ) -> Result<(RegistrySnapshot, usize), (RegistryProviderError, usize)> {
        let mut markets = BTreeMap::new();
        let mut quarantined = Vec::new();
        let mut offset = 0usize;
        let mut attempts = 0usize;

        loop {
            let page = self
                .fetch_page_with_retry(
                    RegistryPageRequest {
                        offset,
                        limit: self.refresh_policy.page_size,
                    },
                    &mut attempts,
                )
                .await?;

            for market in page.markets {
                markets.insert(market.market_id.clone(), market);
            }
            quarantined.extend(page.quarantined);

            match page.next_offset {
                Some(next_offset) => offset = next_offset,
                None => break,
            }
        }

        let mut state = self.state.write().unwrap();
        state.next_sequence += 1;
        let snapshot = RegistrySnapshot {
            provider: self.provider.provider_name().to_string(),
            sequence: state.next_sequence,
            refreshed_at_ms: now_ms(),
            markets,
            quarantined,
        };

        Ok((snapshot, attempts))
    }

    async fn fetch_page_with_retry(
        &self,
        request: RegistryPageRequest,
        attempts: &mut usize,
    ) -> Result<crate::registry_provider::RegistryPage, (RegistryProviderError, usize)> {
        let mut retries = 0usize;

        loop {
            *attempts += 1;
            match self.provider.fetch_page(request.clone()).await {
                Ok(page) => return Ok(page),
                Err(err)
                    if err.is_retryable()
                        && retries < self.refresh_policy.retry_policy.max_retries =>
                {
                    let delay = self.refresh_policy.retry_policy.backoff_for_retry(retries);
                    retries += 1;
                    tokio::time::sleep(delay).await;
                }
                Err(err) => return Err((err, *attempts)),
            }
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;

    use super::*;
    use crate::registry_provider::{RegistryPage, RegistryPageRequest, RegistryProviderError};
    use crate::snapshot::{QuarantinedMarketRecord, UniverseSelectionPolicy};
    use rtt_core::{
        AssetId, MarketId, MarketMeta, MarketStatus, MinOrderSize, Notional, OutcomeSide,
        OutcomeToken, Price, RewardFreshness, RewardParams, Size, TickSize,
    };
    use tokio::task::yield_now;
    use tokio::time::advance;

    #[derive(Clone)]
    struct SequenceProvider {
        provider_name: String,
        responses: Arc<Mutex<VecDeque<Result<RegistryPage, RegistryProviderError>>>>,
        requests: Arc<Mutex<Vec<RegistryPageRequest>>>,
        call_count: Arc<AtomicUsize>,
    }

    impl SequenceProvider {
        fn new(responses: Vec<Result<RegistryPage, RegistryProviderError>>) -> Self {
            Self {
                provider_name: "fixture-registry".to_string(),
                responses: Arc::new(Mutex::new(VecDeque::from(responses))),
                requests: Arc::new(Mutex::new(Vec::new())),
                call_count: Arc::new(AtomicUsize::new(0)),
            }
        }

        fn requests(&self) -> Vec<RegistryPageRequest> {
            self.requests.lock().unwrap().clone()
        }

        fn call_count(&self) -> usize {
            self.call_count.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl crate::registry_provider::RegistryProvider for SequenceProvider {
        fn provider_name(&self) -> &str {
            &self.provider_name
        }

        async fn fetch_page(
            &self,
            request: RegistryPageRequest,
        ) -> Result<RegistryPage, RegistryProviderError> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            self.requests.lock().unwrap().push(request);
            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .expect("queued response")
        }
    }

    fn market(market_id: &str, status: MarketStatus) -> MarketMeta {
        MarketMeta {
            market_id: MarketId::new(market_id),
            yes_asset: OutcomeToken::new(
                AssetId::new(format!("{market_id}-yes")),
                OutcomeSide::Yes,
            ),
            no_asset: OutcomeToken::new(AssetId::new(format!("{market_id}-no")), OutcomeSide::No),
            condition_id: Some(format!("condition-{market_id}")),
            tick_size: TickSize::new("0.01"),
            min_order_size: Some(MinOrderSize::new("5")),
            status,
            reward: None,
        }
    }

    fn page(
        markets: Vec<MarketMeta>,
        quarantined: Vec<QuarantinedMarketRecord>,
        next_offset: Option<usize>,
    ) -> RegistryPage {
        RegistryPage {
            markets,
            quarantined,
            next_offset,
        }
    }

    fn refresh_policy() -> RegistryRefreshPolicy {
        RegistryRefreshPolicy {
            page_size: 2,
            refresh_interval: Duration::from_secs(30),
            retry_policy: RetryPolicy {
                max_retries: 2,
                initial_backoff: Duration::from_millis(50),
                max_backoff: Duration::from_millis(200),
            },
        }
    }

    fn reward_market(
        market_id: &str,
        total_daily_rate: &str,
        competitiveness: &str,
        min_size: &str,
        updated_at_ms: u64,
    ) -> RewardDiscoveryMarket {
        RewardDiscoveryMarket {
            market: MarketMeta {
                market_id: MarketId::new(market_id),
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
                status: MarketStatus::Active,
                reward: Some(RewardParams {
                    rate_bps: None,
                    max_spread: Some(Price::new("0.04")),
                    min_size: Some(Size::new(min_size)),
                    min_notional: None,
                    native_daily_rate: Some(Notional::new(total_daily_rate)),
                    sponsored_daily_rate: None,
                    total_daily_rate: Some(Notional::new(total_daily_rate)),
                    market_competitiveness: Some(competitiveness.to_string()),
                    fee_enabled: Some(false),
                    updated_at_ms: Some(updated_at_ms),
                    freshness: RewardFreshness::Fresh,
                }),
            },
            end_time_ms: Some(1_800_000_000_000),
            accepting_orders: true,
        }
    }

    #[test]
    fn refresh_policy_marks_due_when_interval_elapses() {
        let policy = refresh_policy();

        assert!(policy.is_refresh_due(None, 1_000));
        assert!(!policy.is_refresh_due(Some(1_000), 30_999));
        assert!(policy.is_refresh_due(Some(1_000), 31_000));
    }

    #[tokio::test]
    async fn refresh_paginates_full_snapshot_and_retains_quarantine() {
        let provider = SequenceProvider::new(vec![
            Ok(page(
                vec![
                    market("market-1", MarketStatus::Active),
                    market("market-2", MarketStatus::Active),
                ],
                Vec::new(),
                Some(2),
            )),
            Ok(page(
                vec![market("market-3", MarketStatus::Closed)],
                vec![QuarantinedMarketRecord {
                    record_id: Some("bad-market".to_string()),
                    reason: "missing_yes_no_pair".to_string(),
                }],
                None,
            )),
        ]);

        let registry = MarketRegistry::new(
            provider.clone(),
            refresh_policy(),
            UniverseSelectionPolicy {
                active_only: true,
                ..UniverseSelectionPolicy::default()
            },
        );

        let outcome = registry.refresh_once().await.unwrap();
        let json = serde_json::to_value(&outcome).unwrap();

        assert_eq!(provider.requests().len(), 2);
        assert_eq!(provider.requests()[0].offset, 0);
        assert_eq!(provider.requests()[0].limit, 2);
        assert_eq!(provider.requests()[1].offset, 2);
        assert_eq!(
            json["snapshot"]["provider"],
            serde_json::json!("fixture-registry")
        );
        assert_eq!(json["snapshot"]["markets"].as_object().unwrap().len(), 3);
        assert_eq!(json["snapshot"]["quarantined"].as_array().unwrap().len(), 1);
        assert_eq!(
            json["universe"]["selected_market_ids"],
            serde_json::json!(["market-1", "market-2"])
        );
    }

    #[tokio::test(start_paused = true)]
    async fn refresh_retries_transient_failures_with_backoff() {
        let provider = SequenceProvider::new(vec![
            Err(RegistryProviderError::transient("429 too many requests")),
            Err(RegistryProviderError::transient("502 bad gateway")),
            Ok(page(
                vec![market("market-1", MarketStatus::Active)],
                Vec::new(),
                None,
            )),
        ]);
        let registry = MarketRegistry::new(
            provider.clone(),
            refresh_policy(),
            UniverseSelectionPolicy::default(),
        );

        let handle = tokio::spawn(async move { registry.refresh_once().await.unwrap() });

        yield_now().await;
        assert_eq!(provider.call_count(), 1);

        advance(Duration::from_millis(49)).await;
        yield_now().await;
        assert_eq!(provider.call_count(), 1);

        advance(Duration::from_millis(1)).await;
        yield_now().await;
        assert_eq!(provider.call_count(), 2);

        advance(Duration::from_millis(99)).await;
        yield_now().await;
        assert_eq!(provider.call_count(), 2);

        advance(Duration::from_millis(1)).await;
        yield_now().await;
        assert_eq!(provider.call_count(), 3);

        let outcome = handle.await.unwrap();
        let json = serde_json::to_value(&outcome).unwrap();
        assert_eq!(json["attempts"], serde_json::json!(3));
        assert_eq!(json["degraded"], serde_json::json!(false));
    }

    #[tokio::test]
    async fn refresh_failure_keeps_last_known_good_snapshot_and_universe() {
        let provider = SequenceProvider::new(vec![
            Ok(page(
                vec![market("market-1", MarketStatus::Active)],
                Vec::new(),
                None,
            )),
            Err(RegistryProviderError::transient("upstream timeout")),
            Err(RegistryProviderError::transient("upstream timeout")),
            Err(RegistryProviderError::transient("upstream timeout")),
        ]);
        let registry = MarketRegistry::new(
            provider,
            refresh_policy(),
            UniverseSelectionPolicy {
                active_only: true,
                ..UniverseSelectionPolicy::default()
            },
        );

        let first = registry.refresh_once().await.unwrap();
        let second = registry.refresh_once().await.unwrap();
        let first_json = serde_json::to_value(&first).unwrap();
        let second_json = serde_json::to_value(&second).unwrap();

        assert_eq!(first_json["snapshot"], second_json["snapshot"]);
        assert_eq!(first_json["universe"], second_json["universe"]);
        assert_eq!(second_json["degraded"], serde_json::json!(true));
        assert_eq!(second_json["error"], serde_json::json!("upstream timeout"));
    }

    #[test]
    fn reward_selector_filters_ineligible_markets_deterministically() {
        let now_ms = 1_700_000_000_000;
        let mut stale = reward_market("stale", "5", "2.0", "50", now_ms - 120_000);
        stale.market.reward.as_mut().unwrap().freshness = RewardFreshness::StaleButUsable;

        let mut near_expiry = reward_market("near-expiry", "5", "2.0", "50", now_ms);
        near_expiry.end_time_ms = Some(now_ms + 30_000);

        let over_competitive = reward_market("over-competitive", "5", "25.0", "50", now_ms);
        let selected = reward_market("selected", "5", "2.5", "50", now_ms);

        let selection = select_reward_markets(
            &[stale, near_expiry, over_competitive, selected],
            &RewardSelectionPolicy {
                max_markets: 2,
                max_total_deployed_usd: 100.0,
                base_quote_size: 50.0,
                edge_buffer: 0.02,
                min_total_daily_rate: 1.0,
                max_market_competitiveness: 10.0,
                min_time_to_expiry_secs: 60,
                max_reward_age_ms: 60_000,
            },
            now_ms,
        );

        assert_eq!(selection.selected.len(), 1);
        assert_eq!(selection.selected[0].market.market_id.as_str(), "selected");
        assert_eq!(
            selection.decisions[0].reason,
            RewardSelectionReason::RewardStale
        );
        assert_eq!(
            selection.decisions[1].reason,
            RewardSelectionReason::NearExpiry
        );
        assert_eq!(
            selection.decisions[2].reason,
            RewardSelectionReason::OverCompetitive
        );
        assert_eq!(
            selection.decisions[3].reason,
            RewardSelectionReason::Selected
        );
    }

    #[test]
    fn reward_selector_ranks_by_reward_per_reserved_capital() {
        let now_ms = 1_700_000_000_000;
        let efficient = reward_market("efficient", "6", "2.0", "50", now_ms);
        let capital_heavy = reward_market("capital-heavy", "6", "2.0", "150", now_ms);

        let selection = select_reward_markets(
            &[capital_heavy, efficient],
            &RewardSelectionPolicy {
                max_markets: 2,
                max_total_deployed_usd: 100.0,
                base_quote_size: 50.0,
                edge_buffer: 0.02,
                min_total_daily_rate: 1.0,
                max_market_competitiveness: 10.0,
                min_time_to_expiry_secs: 60,
                max_reward_age_ms: 60_000,
            },
            now_ms,
        );

        assert_eq!(selection.selected.len(), 1);
        assert_eq!(selection.selected[0].market.market_id.as_str(), "efficient");
        assert_eq!(
            selection.decisions[0].reason,
            RewardSelectionReason::DeploymentBudgetExceeded
        );
        assert_eq!(
            selection.decisions[1].reason,
            RewardSelectionReason::Selected
        );
    }

    #[test]
    fn reward_enrichment_merges_current_rates_and_competitiveness_into_snapshot_markets() {
        let discovery = RewardDiscoveryMarket {
            market: market("market-1", MarketStatus::Active),
            end_time_ms: None,
            accepting_orders: true,
        };

        let enriched = enrich_reward_markets(
            &[discovery],
            &[crate::registry_provider::CurrentRewardConfig {
                condition_id: "condition-market-1".to_string(),
                reward: RewardParams {
                    rate_bps: None,
                    max_spread: Some(Price::new("0.045")),
                    min_size: Some(Size::new("50")),
                    min_notional: None,
                    native_daily_rate: Some(Notional::new("5")),
                    sponsored_daily_rate: Some(Notional::new("0.5")),
                    total_daily_rate: Some(Notional::new("5.5")),
                    market_competitiveness: None,
                    fee_enabled: None,
                    updated_at_ms: Some(1_700_000_000_000),
                    freshness: RewardFreshness::Fresh,
                },
            }],
            &[crate::registry_provider::RawRewardMarket {
                condition_id: "condition-market-1".to_string(),
                market_competitiveness: Some("13.40604".to_string()),
                tokens: Vec::new(),
            }],
        );

        let reward = enriched[0].market.reward.as_ref().expect("reward");
        assert_eq!(reward.total_daily_rate.as_ref().unwrap().as_str(), "5.5");
        assert_eq!(reward.market_competitiveness.as_deref(), Some("13.40604"));
    }
}
