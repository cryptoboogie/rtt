use std::time::Duration;
use std::{
    collections::BTreeMap,
    sync::RwLock,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::Serialize;

use crate::{
    registry_provider::{
        RegistryPageRequest, RegistryProvider, RegistryProviderError,
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

impl RetryPolicy {
    fn backoff_for_retry(&self, retry_index: usize) -> Duration {
        let multiplier = 1u32.checked_shl(retry_index as u32).unwrap_or(u32::MAX);
        let delay = self.initial_backoff.saturating_mul(multiplier);
        delay.min(self.max_backoff)
    }
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
    use crate::registry_provider::{
        RegistryPage, RegistryPageRequest, RegistryProviderError,
    };
    use crate::snapshot::{QuarantinedMarketRecord, UniverseSelectionPolicy};
    use rtt_core::{
        AssetId, MarketId, MarketMeta, MarketStatus, MinOrderSize, OutcomeSide, OutcomeToken,
        TickSize,
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
            no_asset: OutcomeToken::new(
                AssetId::new(format!("{market_id}-no")),
                OutcomeSide::No,
            ),
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
        assert_eq!(json["snapshot"]["provider"], serde_json::json!("fixture-registry"));
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
            Ok(page(vec![market("market-1", MarketStatus::Active)], Vec::new(), None)),
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
            Ok(page(vec![market("market-1", MarketStatus::Active)], Vec::new(), None)),
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
}
