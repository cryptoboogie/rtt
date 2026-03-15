use chrono::{DateTime, Utc};
use futures_util::{SinkExt, StreamExt};
use pm_strategy::config::Btc5mParams;
use serde::Deserialize;
use std::collections::{BTreeSet, VecDeque};
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::time::Duration;

use pm_data::Pipeline;
use rtt_core::clob_auth::L2Credentials;
use rtt_core::clob_order::SignedOrderPayload;
use rtt_core::clob_request::{build_order_request_from_bytes, encode_order_payload};
use rtt_core::clob_response::{parse_order_response, OrderResponse};
use rtt_core::clob_signer::{build_order, sign_order};
use rtt_core::clock;
use rtt_core::connection::ConnectionPool;
use rtt_core::trigger::{OrderBookSnapshot, OrderType, Side, TriggerMessage};
use tokio::sync::broadcast;
use tokio::sync::watch;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

use crate::execution::SignerParams;
use crate::journal::{Btc5mJournal, Btc5mJournalRecord};
use crate::safety::{CircuitBreaker, OrderGuard, RateLimiter};

const BINANCE_CHANNEL_CAPACITY: usize = 1024;
const MAX_ADVANCE_STEPS_PER_EVENT: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Btc5mMarket {
    pub slug: String,
    pub market_id: String,
    pub open_ts: u64,
    pub close_ts: u64,
    pub up_token_id: String,
    pub down_token_id: String,
    pub min_order_size: Option<String>,
    pub tick_size: Option<String>,
}

pub struct GammaBtc5mResolver;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarketOutcome {
    Up,
    Down,
}

impl MarketOutcome {
    fn opposite(self) -> Self {
        match self {
            Self::Up => Self::Down,
            Self::Down => Self::Up,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModeCandidate {
    PairedNoSells,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirstLegPurpose {
    Probe,
    BurstPair,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanupReason {
    BinanceStale,
    BinanceReversed,
    PairUnavailable,
    EntryWindowExpired,
    OneSidedDisabled,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BookTop {
    pub bid_price: Option<f64>,
    pub bid_size: Option<f64>,
    pub ask_price: Option<f64>,
    pub ask_size: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BinanceTrade {
    pub price: f64,
    pub timestamp_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct BinanceMarketState {
    pub open_price: Option<f64>,
    pub latest_price: Option<f64>,
    pub latest_trade_ts_ms: Option<u64>,
    pub recent_trades: VecDeque<BinanceTrade>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PendingFirstLeg {
    pub outcome: MarketOutcome,
    pub size: f64,
    pub entry_price: f64,
    pub entry_cost_usd: f64,
    pub purpose: FirstLegPurpose,
    pub created_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct MarketExecutionState {
    pub blocked: bool,
    pub cleanup_mode: bool,
    pub completed: bool,
    pub last_attempt_at_ms: Option<u64>,
    pub mode_candidate: Option<ModeCandidate>,
    pub binance: BinanceMarketState,
    pub pending_first_leg: Option<PendingFirstLeg>,
    pub paired_size: f64,
    pub spent_pair_budget_usd: f64,
    pub spent_single_side_budget_usd: f64,
    pub gross_deployed_usd: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BinanceSignal {
    pub fresh: bool,
    pub open_price: f64,
    pub latest_price: f64,
    pub signed_move_bps: f64,
    pub absolute_move_bps: f64,
    pub recent_move_bps: f64,
}

impl BinanceSignal {
    fn aligned_move_bps(&self, outcome: MarketOutcome) -> f64 {
        match outcome {
            MarketOutcome::Up => self.signed_move_bps,
            MarketOutcome::Down => -self.signed_move_bps,
        }
    }

    fn aligned_recent_move_bps(&self, outcome: MarketOutcome) -> f64 {
        match outcome {
            MarketOutcome::Up => self.recent_move_bps,
            MarketOutcome::Down => -self.recent_move_bps,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct FirstLegPlan {
    pub purpose: FirstLegPurpose,
    pub mode_candidate: ModeCandidate,
    pub outcome: MarketOutcome,
    pub size: String,
    pub price: String,
    pub cost_usd: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SecondLegPlan {
    pub mode_candidate: ModeCandidate,
    pub outcome: MarketOutcome,
    pub size: String,
    pub price: String,
    pub cost_usd: f64,
    pub pair_sum: f64,
    pub expected_profit_usd: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CleanupPlan {
    pub outcome: MarketOutcome,
    pub size: String,
    pub price: String,
    pub estimated_loss_usd: f64,
    pub reason: CleanupReason,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MarketActionPlan {
    BuyFirstLeg(FirstLegPlan),
    BuySecondLeg(SecondLegPlan),
    Cleanup(CleanupPlan),
}

pub struct Btc5mExecutionContext {
    pub dry_run: bool,
    pub order_type: OrderType,
    pub pool: Arc<ConnectionPool>,
    pub creds: L2Credentials,
    pub signer_params: Option<SignerParams>,
    pub circuit_breaker: CircuitBreaker,
    pub rate_limiter: RateLimiter,
    pub order_guard: OrderGuard,
    pub journal: Btc5mJournal,
}

pub struct Btc5mRunner {
    params: Btc5mParams,
    pipeline: Arc<Pipeline>,
    snapshot_rx: broadcast::Receiver<OrderBookSnapshot>,
    resolver_client: reqwest::Client,
    tracked_markets: Vec<TrackedMarket>,
    session_cleanup_loss_usd: f64,
    execution: Btc5mExecutionContext,
}

#[derive(Debug, Clone)]
struct TrackedMarket {
    market: Btc5mMarket,
    up_book: Option<BookTop>,
    down_book: Option<BookTop>,
    execution: MarketExecutionState,
}

#[derive(Debug, Clone)]
struct DispatchAttemptError {
    fatal: bool,
    message: String,
    response: Option<OrderResponse>,
}

#[derive(Debug, Deserialize)]
struct GammaEvent {
    slug: String,
    markets: Vec<GammaMarket>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GammaMarket {
    id: String,
    slug: String,
    outcomes: String,
    clob_token_ids: String,
    end_date: String,
    order_price_min_tick_size: Option<serde_json::Value>,
    order_min_size: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct BinanceAggTradeMessage {
    #[serde(rename = "e")]
    event_type: String,
    #[serde(rename = "E")]
    event_time_ms: u64,
    #[serde(rename = "p")]
    price: String,
    #[serde(rename = "T")]
    trade_time_ms: Option<u64>,
}

impl GammaBtc5mResolver {
    pub async fn fetch_market(
        client: &reqwest::Client,
        slug_prefix: &str,
        open_ts: u64,
    ) -> Result<Option<Btc5mMarket>, String> {
        let slug = format!("{slug_prefix}-{open_ts}");
        let response = client
            .get(format!(
                "https://gamma-api.polymarket.com/events?slug={slug}"
            ))
            .send()
            .await
            .map_err(|err| format!("gamma request failed: {err}"))?;

        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|err| format!("gamma response body read failed: {err}"))?;

        if !status.is_success() {
            return Err(format!("gamma returned {status} for slug {slug}"));
        }

        Self::parse_exact_event_response(&body, slug_prefix, open_ts)
    }

    pub fn parse_exact_event_response(
        body: &str,
        slug_prefix: &str,
        open_ts: u64,
    ) -> Result<Option<Btc5mMarket>, String> {
        let events: Vec<GammaEvent> = serde_json::from_str(body)
            .map_err(|err| format!("failed to parse gamma events: {err}"))?;
        let Some(event) = events.into_iter().next() else {
            return Ok(None);
        };

        let expected_slug = format!("{slug_prefix}-{open_ts}");
        if event.slug != expected_slug {
            return Err(format!(
                "gamma returned unexpected event slug: expected {expected_slug}, got {}",
                event.slug
            ));
        }

        let Some(market) = event
            .markets
            .into_iter()
            .find(|market| market.slug == expected_slug)
        else {
            return Err(format!(
                "gamma event {expected_slug} did not include its matching market"
            ));
        };

        let outcomes: Vec<String> = serde_json::from_str(&market.outcomes)
            .map_err(|err| format!("failed to parse outcomes for {expected_slug}: {err}"))?;
        let token_ids: Vec<String> = serde_json::from_str(&market.clob_token_ids)
            .map_err(|err| format!("failed to parse clobTokenIds for {expected_slug}: {err}"))?;

        if outcomes.len() != token_ids.len() {
            return Err(format!(
                "gamma market {expected_slug} returned mismatched outcomes/token counts"
            ));
        }

        let mut up_token_id = None;
        let mut down_token_id = None;
        for (outcome, token_id) in outcomes.into_iter().zip(token_ids.into_iter()) {
            match outcome.to_ascii_lowercase().as_str() {
                "up" => up_token_id = Some(token_id),
                "down" => down_token_id = Some(token_id),
                _ => {}
            }
        }

        let close_ts = DateTime::parse_from_rfc3339(&market.end_date)
            .map_err(|err| format!("failed to parse gamma endDate for {expected_slug}: {err}"))?
            .with_timezone(&Utc)
            .timestamp() as u64;

        Ok(Some(Btc5mMarket {
            slug: expected_slug,
            market_id: market.id,
            open_ts,
            close_ts,
            up_token_id: up_token_id.ok_or_else(|| {
                format!("gamma market {slug_prefix}-{open_ts} missing Up outcome token")
            })?,
            down_token_id: down_token_id.ok_or_else(|| {
                format!("gamma market {slug_prefix}-{open_ts} missing Down outcome token")
            })?,
            min_order_size: market.order_min_size.map(scalar_to_string),
            tick_size: market.order_price_min_tick_size.map(scalar_to_string),
        }))
    }
}

impl Btc5mRunner {
    pub async fn new(
        params: Btc5mParams,
        ws_channel_capacity: usize,
        snapshot_channel_capacity: usize,
        execution: Btc5mExecutionContext,
    ) -> Result<Self, String> {
        let resolver_client = reqwest::Client::new();
        let tracked_markets = resolve_tracked_markets(&resolver_client, &params, now_ts()).await?;
        let pipeline = Arc::new(Pipeline::new(
            tracked_market_assets(&tracked_markets),
            ws_channel_capacity,
            snapshot_channel_capacity,
        ));
        let snapshot_rx = pipeline.subscribe_snapshots();

        Ok(Self {
            params,
            pipeline,
            snapshot_rx,
            resolver_client,
            tracked_markets,
            session_cleanup_loss_usd: 0.0,
            execution,
        })
    }

    pub fn ws_last_message_at(&self) -> Arc<AtomicU64> {
        self.pipeline.ws_client_last_message_at()
    }

    pub fn ws_reconnect_count(&self) -> Arc<AtomicU64> {
        self.pipeline.ws_client_reconnect_count()
    }

    pub fn monitored_assets(&self) -> Vec<String> {
        tracked_market_assets(&self.tracked_markets)
    }

    fn record_first_leg_event(
        &self,
        tracked_index: usize,
        plan: &FirstLegPlan,
        action_status: &str,
        response: Option<&OrderResponse>,
        error_message: Option<String>,
    ) {
        let token_id = self.tracked_markets[tracked_index]
            .token_id_for_outcome(plan.outcome)
            .ok();
        self.record_journal_event(
            tracked_index,
            "first_leg",
            action_status,
            Some(first_leg_purpose_label(plan.purpose).to_string()),
            Some(mode_candidate_label(plan.mode_candidate).to_string()),
            Some(outcome_label(plan.outcome).to_string()),
            Some("buy".to_string()),
            token_id,
            Some(plan.size.clone()),
            Some(plan.price.clone()),
            Some(plan.cost_usd),
            None,
            None,
            None,
            None,
            None,
            response,
            error_message,
        );
    }

    fn record_second_leg_event(
        &self,
        tracked_index: usize,
        plan: &SecondLegPlan,
        action_status: &str,
        response: Option<&OrderResponse>,
        error_message: Option<String>,
    ) {
        let token_id = self.tracked_markets[tracked_index]
            .token_id_for_outcome(plan.outcome)
            .ok();
        self.record_journal_event(
            tracked_index,
            "second_leg",
            action_status,
            None,
            Some(mode_candidate_label(plan.mode_candidate).to_string()),
            Some(outcome_label(plan.outcome).to_string()),
            Some("buy".to_string()),
            token_id,
            Some(plan.size.clone()),
            Some(plan.price.clone()),
            Some(plan.cost_usd),
            Some(plan.pair_sum),
            Some(plan.expected_profit_usd),
            None,
            None,
            None,
            response,
            error_message,
        );
    }

    fn record_cleanup_event(
        &self,
        tracked_index: usize,
        plan: &CleanupPlan,
        action_status: &str,
        cleanup_loss_usd: Option<f64>,
        session_cleanup_loss_usd: Option<f64>,
        response: Option<&OrderResponse>,
        error_message: Option<String>,
    ) {
        let token_id = self.tracked_markets[tracked_index]
            .token_id_for_outcome(plan.outcome)
            .ok();
        self.record_journal_event(
            tracked_index,
            "cleanup",
            action_status,
            None,
            None,
            Some(outcome_label(plan.outcome).to_string()),
            Some("sell".to_string()),
            token_id,
            Some(plan.size.clone()),
            Some(plan.price.clone()),
            None,
            None,
            None,
            Some(cleanup_reason_label(plan.reason).to_string()),
            cleanup_loss_usd,
            session_cleanup_loss_usd,
            response,
            error_message,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn record_journal_event(
        &self,
        tracked_index: usize,
        action_kind: &str,
        action_status: &str,
        purpose: Option<String>,
        mode_candidate: Option<String>,
        outcome: Option<String>,
        side: Option<String>,
        token_id: Option<String>,
        size: Option<String>,
        price: Option<String>,
        cost_usd: Option<f64>,
        pair_sum: Option<f64>,
        expected_profit_usd: Option<f64>,
        cleanup_reason: Option<String>,
        cleanup_loss_usd: Option<f64>,
        session_cleanup_loss_usd: Option<f64>,
        response: Option<&OrderResponse>,
        error_message: Option<String>,
    ) {
        let Some(tracked) = self.tracked_markets.get(tracked_index) else {
            return;
        };
        let recorded_at_ms = now_ms();
        let signal = compute_binance_signal(&tracked.execution, &self.params, recorded_at_ms);
        let (order_id, order_status, transaction_hashes_json, trade_ids_json) =
            response_fields(response);

        self.execution.journal.record(Btc5mJournalRecord {
            recorded_at_ms,
            market_slug: tracked.market.slug.clone(),
            market_id: tracked.market.market_id.clone(),
            market_open_ts: tracked.market.open_ts,
            market_close_ts: tracked.market.close_ts,
            up_token_id: tracked.market.up_token_id.clone(),
            down_token_id: tracked.market.down_token_id.clone(),
            dry_run: self.execution.dry_run,
            action_kind: action_kind.to_string(),
            action_status: action_status.to_string(),
            purpose,
            mode_candidate,
            outcome,
            side,
            token_id,
            size,
            price,
            cost_usd,
            pair_sum,
            expected_profit_usd,
            cleanup_reason,
            cleanup_loss_usd,
            session_cleanup_loss_usd,
            binance_open_price: tracked.execution.binance.open_price,
            binance_latest_price: tracked.execution.binance.latest_price,
            binance_signed_move_bps: signal.as_ref().map(|value| value.signed_move_bps),
            binance_recent_move_bps: signal.as_ref().map(|value| value.recent_move_bps),
            order_id,
            order_status,
            transaction_hashes_json,
            trade_ids_json,
            error_message,
        });
    }

    pub async fn run(&mut self, mut shutdown: watch::Receiver<bool>) -> Result<(), String> {
        let pipeline = self.pipeline.clone();
        let pipeline_run = async move {
            pipeline.run().await;
        };
        tokio::pin!(pipeline_run);

        let (binance_tx, _) = broadcast::channel(BINANCE_CHANNEL_CAPACITY);
        let mut binance_rx = binance_tx.subscribe();
        let binance_run = run_binance_feed(
            self.params.binance_ws_url.clone(),
            binance_tx,
            shutdown.clone(),
        );
        tokio::pin!(binance_run);

        let mut refresh_interval = tokio::time::interval(Duration::from_secs(1));
        refresh_interval.tick().await;

        loop {
            tokio::select! {
                _ = &mut pipeline_run => {
                    return Err("polymarket pipeline stopped".to_string());
                }
                result = &mut binance_run => {
                    result?;
                    return Err("binance feed stopped".to_string());
                }
                _ = refresh_interval.tick() => {
                    self.refresh_market_window().await?;
                }
                result = self.snapshot_rx.recv() => {
                    match result {
                        Ok(snapshot) => self.handle_snapshot(snapshot).await?,
                        Err(broadcast::error::RecvError::Lagged(skipped)) => {
                            tracing::warn!(skipped, "BTC 5m runner lagged on Polymarket snapshot stream");
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            return Err("btc 5m snapshot stream closed".to_string());
                        }
                    }
                }
                result = binance_rx.recv() => {
                    match result {
                        Ok(trade) => self.handle_binance_trade(trade).await?,
                        Err(broadcast::error::RecvError::Lagged(skipped)) => {
                            tracing::warn!(skipped, "BTC 5m runner lagged on Binance trade stream");
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            return Err("btc 5m binance trade stream closed".to_string());
                        }
                    }
                }
                _ = shutdown.changed() => {
                    if *shutdown.borrow() {
                        self.pipeline.shutdown();
                        let _ = (&mut pipeline_run).await;
                        let _ = (&mut binance_run).await;
                        return Ok(());
                    }
                }
            }
        }
    }

    async fn refresh_market_window(&mut self) -> Result<(), String> {
        let desired_open_times: BTreeSet<u64> = desired_open_times(
            now_ts(),
            self.params.cadence_seconds,
            self.params.prefetch_markets,
        )
        .into_iter()
        .collect();

        self.tracked_markets.retain(|tracked| {
            desired_open_times.contains(&tracked.market.open_ts)
                || tracked.execution.pending_first_leg.is_some()
                || tracked.execution.cleanup_mode
        });

        let existing_open_times: BTreeSet<u64> = self
            .tracked_markets
            .iter()
            .map(|tracked| tracked.market.open_ts)
            .collect();

        for open_ts in desired_open_times {
            if existing_open_times.contains(&open_ts) {
                continue;
            }

            if let Some(market) = GammaBtc5mResolver::fetch_market(
                &self.resolver_client,
                &self.params.market_slug_prefix,
                open_ts,
            )
            .await?
            {
                self.tracked_markets.push(TrackedMarket::new(market));
            }
        }

        self.tracked_markets
            .sort_by_key(|tracked| tracked.market.open_ts);
        self.pipeline
            .reconfigure_assets(tracked_market_assets(&self.tracked_markets));
        Ok(())
    }

    async fn handle_snapshot(&mut self, snapshot: OrderBookSnapshot) -> Result<(), String> {
        let now_ms = now_ms();

        for index in 0..self.tracked_markets.len() {
            let matched = if snapshot.asset_id == self.tracked_markets[index].market.up_token_id {
                self.tracked_markets[index].up_book = Some(book_top_from_snapshot(&snapshot));
                true
            } else if snapshot.asset_id == self.tracked_markets[index].market.down_token_id {
                self.tracked_markets[index].down_book = Some(book_top_from_snapshot(&snapshot));
                true
            } else {
                false
            };

            if matched {
                self.advance_market(index, now_ms).await?;
                break;
            }
        }

        Ok(())
    }

    async fn handle_binance_trade(&mut self, trade: BinanceTrade) -> Result<(), String> {
        let now_ms = trade.timestamp_ms.max(now_ms());
        let indices: Vec<usize> = (0..self.tracked_markets.len()).collect();

        for index in indices {
            let open_ts_ms = self.tracked_markets[index].market.open_ts * 1000;
            apply_binance_trade_to_state(
                &mut self.tracked_markets[index].execution,
                &trade,
                open_ts_ms,
                self.params.binance_buffer_window_ms,
            );
            self.advance_market(index, now_ms).await?;
        }

        Ok(())
    }

    async fn advance_market(&mut self, tracked_index: usize, now_ms: u64) -> Result<(), String> {
        for _ in 0..MAX_ADVANCE_STEPS_PER_EVENT {
            let Some(plan) = self.plan_market_action(tracked_index, now_ms) else {
                return Ok(());
            };

            let should_continue = self.execute_market_action(tracked_index, plan).await?;
            if !should_continue {
                return Ok(());
            }
        }

        tracing::warn!(
            market = %self.tracked_markets[tracked_index].market.slug,
            "BTC 5m market advance loop hit safety cap"
        );
        Ok(())
    }

    fn plan_market_action(
        &self,
        tracked_index: usize,
        now_ms: u64,
    ) -> Option<MarketActionPlan> {
        let tracked = self.tracked_markets.get(tracked_index)?;
        plan_market_action(
            &tracked.market,
            &self.params,
            &tracked.execution,
            now_ms,
            tracked.up_book.as_ref(),
            tracked.down_book.as_ref(),
        )
    }

    async fn execute_market_action(
        &mut self,
        tracked_index: usize,
        plan: MarketActionPlan,
    ) -> Result<bool, String> {
        match plan {
            MarketActionPlan::BuyFirstLeg(plan) => {
                let trigger = self.order_trigger(
                    tracked_index,
                    plan.outcome,
                    Side::Buy,
                    &plan.price,
                    &plan.size,
                )?;
                match self.dispatch_trigger(&trigger).await {
                    Ok(response) => {
                        let size = parse_f64(&plan.size).unwrap_or(0.0);
                        let price = parse_f64(&plan.price).unwrap_or(0.0);
                        {
                            let state = &mut self.tracked_markets[tracked_index].execution;
                            state.pending_first_leg = Some(PendingFirstLeg {
                                outcome: plan.outcome,
                                size,
                                entry_price: price,
                                entry_cost_usd: plan.cost_usd,
                                purpose: plan.purpose,
                                created_at_ms: now_ms(),
                            });
                            state.last_attempt_at_ms = Some(now_ms());
                            state.mode_candidate = Some(plan.mode_candidate);
                            state.cleanup_mode = false;
                            state.gross_deployed_usd += plan.cost_usd;
                        }

                        self.record_first_leg_event(
                            tracked_index,
                            &plan,
                            "matched",
                            Some(&response),
                            None,
                        );

                        tracing::info!(
                            market = %self.tracked_markets[tracked_index].market.slug,
                            purpose = ?plan.purpose,
                            mode = ?plan.mode_candidate,
                            outcome = ?plan.outcome,
                            order_id = %response.order_id,
                            status = %response.status,
                            size = %plan.size,
                            price = %plan.price,
                            cost_usd = plan.cost_usd,
                            "BTC 5m first leg matched"
                        );

                        Ok(true)
                    }
                    Err(error) if !error.fatal => {
                        self.record_first_leg_event(
                            tracked_index,
                            &plan,
                            "not_matched",
                            error.response.as_ref(),
                            Some(error.message.clone()),
                        );
                        tracing::warn!(
                            market = %self.tracked_markets[tracked_index].market.slug,
                            purpose = ?plan.purpose,
                            outcome = ?plan.outcome,
                            error = %error.message,
                            "BTC 5m first leg did not match"
                        );
                        Ok(false)
                    }
                    Err(error) => {
                        self.record_first_leg_event(
                            tracked_index,
                            &plan,
                            "fatal_error",
                            error.response.as_ref(),
                            Some(error.message.clone()),
                        );
                        Err(error.message)
                    }
                }
            }
            MarketActionPlan::BuySecondLeg(plan) => {
                let trigger = self.order_trigger(
                    tracked_index,
                    plan.outcome,
                    Side::Buy,
                    &plan.price,
                    &plan.size,
                )?;
                match self.dispatch_trigger(&trigger).await {
                    Ok(response) => {
                        let market_slug = self.tracked_markets[tracked_index].market.slug.clone();
                        let size = parse_f64(&plan.size).unwrap_or(0.0);
                        let (paired_size, spent_pair_budget_usd);
                        {
                            let state = &mut self.tracked_markets[tracked_index].execution;
                            let Some(pending) = state.pending_first_leg.take() else {
                                return Ok(false);
                            };
                            state.last_attempt_at_ms = Some(now_ms());
                            state.cleanup_mode = false;
                            state.paired_size += size;
                            state.spent_pair_budget_usd += pending.entry_cost_usd + plan.cost_usd;
                            state.gross_deployed_usd += plan.cost_usd;
                            if state.spent_pair_budget_usd + 1e-9 >= self.params.max_pair_budget_usd {
                                state.completed = true;
                            }
                            paired_size = state.paired_size;
                            spent_pair_budget_usd = state.spent_pair_budget_usd;
                        }

                        self.record_second_leg_event(
                            tracked_index,
                            &plan,
                            "matched",
                            Some(&response),
                            None,
                        );

                        tracing::info!(
                            market = %market_slug,
                            mode = ?plan.mode_candidate,
                            order_id = %response.order_id,
                            status = %response.status,
                            size = %plan.size,
                            price = %plan.price,
                            pair_sum = plan.pair_sum,
                            expected_profit_usd = plan.expected_profit_usd,
                            paired_size = paired_size,
                            spent_pair_budget_usd = spent_pair_budget_usd,
                            "BTC 5m pair leg matched"
                        );

                        Ok(true)
                    }
                    Err(error) if !error.fatal => {
                        self.tracked_markets[tracked_index].execution.cleanup_mode = true;
                        self.tracked_markets[tracked_index].execution.last_attempt_at_ms = Some(now_ms());

                        self.record_second_leg_event(
                            tracked_index,
                            &plan,
                            "not_matched",
                            error.response.as_ref(),
                            Some(error.message.clone()),
                        );

                        tracing::warn!(
                            market = %self.tracked_markets[tracked_index].market.slug,
                            outcome = ?plan.outcome,
                            error = %error.message,
                            "BTC 5m second leg failed, entering cleanup mode"
                        );
                        Ok(true)
                    }
                    Err(error) => {
                        self.record_second_leg_event(
                            tracked_index,
                            &plan,
                            "fatal_error",
                            error.response.as_ref(),
                            Some(error.message.clone()),
                        );
                        Err(error.message)
                    }
                }
            }
            MarketActionPlan::Cleanup(plan) => {
                let trigger = self.order_trigger(
                    tracked_index,
                    plan.outcome,
                    Side::Sell,
                    &plan.price,
                    &plan.size,
                )?;
                match self.dispatch_trigger(&trigger).await {
                    Ok(response) => {
                        let cleanup_price = parse_f64(&plan.price).unwrap_or(0.0);
                        let cleanup_loss_usd;
                        {
                            let state = &mut self.tracked_markets[tracked_index].execution;
                            let Some(pending) = state.pending_first_leg.take() else {
                                return Ok(false);
                            };
                            cleanup_loss_usd =
                                (pending.entry_price - cleanup_price).max(0.0) * pending.size;
                            self.session_cleanup_loss_usd = next_cleanup_loss_total(
                                self.session_cleanup_loss_usd,
                                cleanup_loss_usd,
                            );
                            state.last_attempt_at_ms = Some(now_ms());
                            state.cleanup_mode = true;
                            state.blocked = true;
                            state.completed = true;
                            state.spent_single_side_budget_usd += pending.entry_cost_usd;
                        }

                        self.record_cleanup_event(
                            tracked_index,
                            &plan,
                            "matched",
                            Some(cleanup_loss_usd),
                            Some(self.session_cleanup_loss_usd),
                            Some(&response),
                            None,
                        );

                        tracing::warn!(
                            market = %self.tracked_markets[tracked_index].market.slug,
                            reason = ?plan.reason,
                            order_id = %response.order_id,
                            status = %response.status,
                            cleanup_loss_usd = cleanup_loss_usd,
                            session_cleanup_loss_usd = self.session_cleanup_loss_usd,
                            "BTC 5m cleanup matched"
                        );

                        if self.session_cleanup_loss_usd >= self.params.max_cleanup_loss_usd {
                            self.execution.circuit_breaker.trip();
                            return Err(format!(
                                "btc 5m cleanup loss stop hit: ${:.2}/${:.2}",
                                self.session_cleanup_loss_usd, self.params.max_cleanup_loss_usd
                            ));
                        }

                        Ok(false)
                    }
                    Err(error) if !error.fatal => {
                        self.tracked_markets[tracked_index].execution.cleanup_mode = true;
                        self.tracked_markets[tracked_index].execution.last_attempt_at_ms = Some(now_ms());
                        self.record_cleanup_event(
                            tracked_index,
                            &plan,
                            "not_matched",
                            None,
                            Some(self.session_cleanup_loss_usd),
                            error.response.as_ref(),
                            Some(error.message.clone()),
                        );
                        tracing::warn!(
                            market = %self.tracked_markets[tracked_index].market.slug,
                            reason = ?plan.reason,
                            error = %error.message,
                            "BTC 5m cleanup attempt did not match"
                        );
                        Ok(false)
                    }
                    Err(error) => {
                        self.record_cleanup_event(
                            tracked_index,
                            &plan,
                            "fatal_error",
                            None,
                            Some(self.session_cleanup_loss_usd),
                            error.response.as_ref(),
                            Some(error.message.clone()),
                        );
                        Err(error.message)
                    }
                }
            }
        }
    }

    fn order_trigger(
        &self,
        tracked_index: usize,
        outcome: MarketOutcome,
        side: Side,
        price: &str,
        size: &str,
    ) -> Result<TriggerMessage, String> {
        let tracked = self
            .tracked_markets
            .get(tracked_index)
            .ok_or_else(|| format!("tracked market {tracked_index} not found"))?;
        let token_id = tracked.token_id_for_outcome(outcome)?;

        Ok(TriggerMessage {
            trigger_id: now_ms(),
            token_id,
            side,
            price: price.to_string(),
            size: size.to_string(),
            order_type: self.execution.order_type,
            timestamp_ns: clock::now_ns(),
        })
    }

    async fn dispatch_trigger(
        &self,
        trigger: &TriggerMessage,
    ) -> Result<OrderResponse, DispatchAttemptError> {
        if self.execution.circuit_breaker.is_tripped() {
            return Err(DispatchAttemptError {
                fatal: true,
                message: "circuit breaker already tripped".to_string(),
                response: None,
            });
        }

        if !self.execution.rate_limiter.try_acquire() {
            return Err(DispatchAttemptError {
                fatal: false,
                message: "rate limit exceeded".to_string(),
                response: None,
            });
        }

        if !self.execution.order_guard.try_acquire() {
            return Err(DispatchAttemptError {
                fatal: false,
                message: "order already in flight".to_string(),
                response: None,
            });
        }

        let result = self.dispatch_trigger_inner(trigger).await;
        self.execution.order_guard.release();
        result
    }

    async fn dispatch_trigger_inner(
        &self,
        trigger: &TriggerMessage,
    ) -> Result<OrderResponse, DispatchAttemptError> {
        if self.execution.dry_run {
            tracing::info!(
                market_token = %trigger.token_id,
                side = ?trigger.side,
                price = %trigger.price,
                size = %trigger.size,
                "[DRY RUN] BTC 5m order would fire"
            );
            return Ok(OrderResponse {
                success: true,
                order_id: "dry-run".to_string(),
                status: "matched".to_string(),
                transaction_hashes: Vec::new(),
                trade_ids: Vec::new(),
                error_msg: None,
            });
        }

        self.execution
            .circuit_breaker
            .check_and_record(&trigger.price, &trigger.size)
            .map_err(|err| DispatchAttemptError {
                fatal: true,
                message: err.to_string(),
                response: None,
            })?;

        let signer = self
            .execution
            .signer_params
            .as_ref()
            .ok_or_else(|| DispatchAttemptError {
                fatal: true,
                message: "signer params required for live btc_5m execution".to_string(),
                response: None,
            })?;

        let order = build_order(
            trigger,
            signer.maker,
            signer.signer_addr,
            signer.fee_rate_bps,
            signer.sig_type,
        )
        .map_err(|err| DispatchAttemptError {
            fatal: true,
            message: format!("failed to build btc_5m order: {err}"),
            response: None,
        })?;

        let signature = sign_order(&signer.signer, &order, signer.is_neg_risk)
            .await
            .map_err(|err| DispatchAttemptError {
                fatal: true,
                message: format!("failed to sign btc_5m order: {err}"),
                response: None,
            })?;

        let payload = SignedOrderPayload::new(&order, &signature, trigger.order_type, &signer.owner);
        let body = encode_order_payload(&payload).map_err(|err| DispatchAttemptError {
            fatal: true,
            message: format!("failed to encode btc_5m order payload: {err}"),
            response: None,
        })?;
        let request =
            build_order_request_from_bytes(body, &self.execution.creds).map_err(|err| {
                DispatchAttemptError {
                    fatal: true,
                    message: format!("failed to build btc_5m order request: {err}"),
                    response: None,
                }
            })?;

        let handle = self
            .execution
            .pool
            .send_start(request)
            .await
            .map_err(|err| DispatchAttemptError {
                fatal: true,
                message: format!("failed to dispatch btc_5m order: {err}"),
                response: None,
            })?;
        let response = self
            .execution
            .pool
            .collect(handle)
            .await
            .map_err(|err| DispatchAttemptError {
                fatal: true,
                message: format!("failed to collect btc_5m order response: {err}"),
                response: None,
            })?;

        let order_response = parse_order_response(response.into_body().as_ref()).map_err(|err| {
            DispatchAttemptError {
                fatal: true,
                message: format!("failed to parse btc_5m order response: {err}"),
                response: None,
            }
        })?;

        if order_response.success && order_response.status.eq_ignore_ascii_case("matched") {
            Ok(order_response)
        } else {
            Err(DispatchAttemptError {
                fatal: false,
                message: order_response
                    .error_msg
                    .clone()
                    .unwrap_or_else(|| format!("unexpected order status {}", order_response.status)),
                response: Some(order_response),
            })
        }
    }
}

impl TrackedMarket {
    fn new(market: Btc5mMarket) -> Self {
        Self {
            market,
            up_book: None,
            down_book: None,
            execution: MarketExecutionState::default(),
        }
    }

    fn token_id_for_outcome(&self, outcome: MarketOutcome) -> Result<String, String> {
        Ok(match outcome {
            MarketOutcome::Up => self.market.up_token_id.clone(),
            MarketOutcome::Down => self.market.down_token_id.clone(),
        })
    }
}

pub fn active_market_open_ts(now_ts: u64, cadence_seconds: u64) -> u64 {
    now_ts - (now_ts % cadence_seconds)
}

pub fn parse_binance_trade_message(text: &str) -> Option<BinanceTrade> {
    let wire: BinanceAggTradeMessage = serde_json::from_str(text).ok()?;
    if wire.event_type != "aggTrade" {
        return None;
    }

    Some(BinanceTrade {
        price: parse_f64(&wire.price)?,
        timestamp_ms: wire.trade_time_ms.unwrap_or(wire.event_time_ms),
    })
}

pub fn apply_binance_trade_to_state(
    state: &mut MarketExecutionState,
    trade: &BinanceTrade,
    market_open_ts_ms: u64,
    buffer_window_ms: u64,
) {
    if trade.timestamp_ms < market_open_ts_ms {
        return;
    }

    if state.binance.open_price.is_none() {
        state.binance.open_price = Some(trade.price);
    }
    state.binance.latest_price = Some(trade.price);
    state.binance.latest_trade_ts_ms = Some(trade.timestamp_ms);
    state.binance.recent_trades.push_back(trade.clone());

    let cutoff = trade.timestamp_ms.saturating_sub(buffer_window_ms);
    while let Some(front) = state.binance.recent_trades.front() {
        if front.timestamp_ms < cutoff {
            state.binance.recent_trades.pop_front();
        } else {
            break;
        }
    }
}

pub fn compute_binance_signal(
    state: &MarketExecutionState,
    params: &Btc5mParams,
    now_ms: u64,
) -> Option<BinanceSignal> {
    let open_price = state.binance.open_price?;
    let latest_price = state.binance.latest_price?;
    let latest_trade_ts_ms = state.binance.latest_trade_ts_ms?;
    let fresh = now_ms.saturating_sub(latest_trade_ts_ms) <= params.binance_stale_after_ms;

    let recent_cutoff = latest_trade_ts_ms.saturating_sub(params.binance_continuation_window_ms);
    let recent_open = state
        .binance
        .recent_trades
        .iter()
        .find(|trade| trade.timestamp_ms >= recent_cutoff)
        .map(|trade| trade.price)
        .unwrap_or(latest_price);

    let signed_move_bps = bps_change(open_price, latest_price);
    let recent_move_bps = bps_change(recent_open, latest_price);

    Some(BinanceSignal {
        fresh,
        open_price,
        latest_price,
        signed_move_bps,
        absolute_move_bps: signed_move_bps.abs(),
        recent_move_bps,
    })
}

pub fn plan_market_action(
    market: &Btc5mMarket,
    params: &Btc5mParams,
    state: &MarketExecutionState,
    now_ms: u64,
    up_book: Option<&BookTop>,
    down_book: Option<&BookTop>,
) -> Option<MarketActionPlan> {
    if state.blocked || state.completed {
        return None;
    }

    if let Some(last_attempt_at_ms) = state.last_attempt_at_ms {
        if now_ms.saturating_sub(last_attempt_at_ms) < params.attempt_cooldown_ms {
            return None;
        }
    }

    if state.pending_first_leg.is_some() {
        if let Some(plan) = plan_cleanup(market, params, state, now_ms, up_book, down_book) {
            return Some(MarketActionPlan::Cleanup(plan));
        }
        return plan_second_leg(market, params, state, now_ms, up_book, down_book)
            .map(MarketActionPlan::BuySecondLeg);
    }

    let offset_seconds = seconds_since_open(market, now_ms);
    if offset_seconds < params.entry_window_start_seconds
        || offset_seconds > params.probe_window_end_seconds
    {
        return None;
    }

    let signal = compute_binance_signal(state, params, now_ms)?;
    if !signal.fresh || signal.absolute_move_bps < params.binance_min_move_bps {
        return None;
    }

    let first_outcome = preferred_first_outcome(&signal)?;
    let pair_sum = pair_sum_from_books(up_book, down_book)?;
    if pair_sum <= 0.0 || pair_sum > params.carry_pair_sum_max {
        return None;
    }

    let first_book = book_for_outcome(up_book, down_book, first_outcome)?;
    let second_book = book_for_outcome(up_book, down_book, first_outcome.opposite())?;
    let first_ask_price = first_book.ask_price?;
    let first_ask_size = first_book.ask_size?;
    let second_ask_size = second_book.ask_size?;
    let min_order_size = market_min_order_size(market);

    let purpose = if state.paired_size > 0.0 {
        FirstLegPurpose::BurstPair
    } else {
        FirstLegPurpose::Probe
    };

    let size = match purpose {
        FirstLegPurpose::Probe => {
            let remaining_pair_budget =
                (params.max_pair_budget_usd - state.spent_pair_budget_usd).max(0.0);
            let remaining_gross =
                (params.max_gross_deployed_per_market - state.gross_deployed_usd).max(0.0);
            let probe_budget = params
                .probe_budget_usd
                .min(params.max_unpaired_exposure_usd)
                .min(remaining_pair_budget)
                .min(remaining_gross);
            plan_shares_from_one_sided_budget(probe_budget, first_ask_price, first_ask_size, min_order_size)
        }
        FirstLegPurpose::BurstPair => {
            let remaining_pair_budget =
                (params.max_pair_budget_usd - state.spent_pair_budget_usd).max(0.0);
            let remaining_gross =
                (params.max_gross_deployed_per_market - state.gross_deployed_usd).max(0.0);
            let burst_budget = params
                .initial_burst_budget_usd
                .min(remaining_pair_budget)
                .min(remaining_gross);
            plan_shares_from_pair_budget(
                burst_budget,
                pair_sum,
                first_ask_size,
                second_ask_size,
                min_order_size,
            )
        }
    }?;

    Some(FirstLegPlan {
        purpose,
        mode_candidate: ModeCandidate::PairedNoSells,
        outcome: first_outcome,
        size: format_decimal(size),
        price: format_decimal(first_ask_price),
        cost_usd: size * first_ask_price,
    }
    .into())
}

fn plan_second_leg(
    market: &Btc5mMarket,
    params: &Btc5mParams,
    state: &MarketExecutionState,
    now_ms: u64,
    up_book: Option<&BookTop>,
    down_book: Option<&BookTop>,
) -> Option<SecondLegPlan> {
    let pending = state.pending_first_leg.as_ref()?;
    let offset_seconds = seconds_since_open(market, now_ms);
    if offset_seconds > params.entry_window_end_seconds {
        return None;
    }

    let signal = compute_binance_signal(state, params, now_ms)?;
    if !signal.fresh {
        return None;
    }

    let second_outcome = pending.outcome.opposite();
    let second_book = book_for_outcome(up_book, down_book, second_outcome)?;
    let second_ask_price = second_book.ask_price?;
    let second_ask_size = second_book.ask_size?;
    if second_ask_size < pending.size {
        return None;
    }

    let pair_sum = pending.entry_price + second_ask_price;
    if pair_sum > params.carry_pair_sum_max || pair_sum <= 0.0 {
        return None;
    }

    let second_cost_usd = pending.size * second_ask_price;
    let remaining_pair_budget = (params.max_pair_budget_usd - state.spent_pair_budget_usd).max(0.0);
    if pending.entry_cost_usd + second_cost_usd > remaining_pair_budget + 1e-9 {
        return None;
    }

    let remaining_gross =
        (params.max_gross_deployed_per_market - state.gross_deployed_usd).max(0.0);
    if second_cost_usd > remaining_gross + 1e-9 {
        return None;
    }

    Some(SecondLegPlan {
        mode_candidate: state.mode_candidate.unwrap_or(ModeCandidate::PairedNoSells),
        outcome: second_outcome,
        size: format_decimal(pending.size),
        price: format_decimal(second_ask_price),
        cost_usd: second_cost_usd,
        pair_sum,
        expected_profit_usd: pending.size * (1.0 - pair_sum),
    })
}

fn plan_cleanup(
    market: &Btc5mMarket,
    params: &Btc5mParams,
    state: &MarketExecutionState,
    now_ms: u64,
    up_book: Option<&BookTop>,
    down_book: Option<&BookTop>,
) -> Option<CleanupPlan> {
    let pending = state.pending_first_leg.as_ref()?;
    let reason = cleanup_reason(market, params, state, now_ms, up_book, down_book)?;
    let book = book_for_outcome(up_book, down_book, pending.outcome)?;
    let bid_price = book.bid_price?;
    let bid_size = book.bid_size?;
    if bid_size < pending.size {
        return None;
    }

    Some(CleanupPlan {
        outcome: pending.outcome,
        size: format_decimal(pending.size),
        price: format_decimal(bid_price),
        estimated_loss_usd: (pending.entry_price - bid_price).max(0.0) * pending.size,
        reason,
    })
}

fn cleanup_reason(
    market: &Btc5mMarket,
    params: &Btc5mParams,
    state: &MarketExecutionState,
    now_ms: u64,
    up_book: Option<&BookTop>,
    down_book: Option<&BookTop>,
) -> Option<CleanupReason> {
    let pending = state.pending_first_leg.as_ref()?;
    if state.cleanup_mode {
        return Some(CleanupReason::PairUnavailable);
    }

    let offset_seconds = seconds_since_open(market, now_ms);
    if offset_seconds > params.entry_window_end_seconds {
        return Some(CleanupReason::EntryWindowExpired);
    }

    let signal = compute_binance_signal(state, params, now_ms)?;
    if !signal.fresh {
        return Some(CleanupReason::BinanceStale);
    }

    if signal.aligned_move_bps(pending.outcome) <= -params.binance_reversal_veto_bps
        || signal.aligned_recent_move_bps(pending.outcome) <= -params.binance_reversal_veto_bps
    {
        return Some(CleanupReason::BinanceReversed);
    }

    let pair_attractive = plan_second_leg(market, params, state, now_ms, up_book, down_book).is_some();
    if pair_attractive {
        return None;
    }

    let pending_age_ms = now_ms.saturating_sub(pending.created_at_ms);
    if pending_age_ms < params.cleanup_grace_ms {
        return None;
    }

    if params.allow_one_sided_continuation
        && one_sided_eligible(state, params, now_ms, pending.outcome)
        && pending.entry_cost_usd <= params.max_single_side_budget_usd + 1e-9
    {
        return None;
    }

    Some(if params.allow_one_sided_continuation {
        CleanupReason::PairUnavailable
    } else {
        CleanupReason::OneSidedDisabled
    })
}

fn one_sided_eligible(
    state: &MarketExecutionState,
    params: &Btc5mParams,
    now_ms: u64,
    outcome: MarketOutcome,
) -> bool {
    let Some(signal) = compute_binance_signal(state, params, now_ms) else {
        return false;
    };

    signal.fresh
        && signal.aligned_move_bps(outcome) >= params.one_sided_min_aligned_entry_bps
        && signal.aligned_recent_move_bps(outcome) > 0.0
}

fn preferred_first_outcome(signal: &BinanceSignal) -> Option<MarketOutcome> {
    if signal.signed_move_bps > 0.0 {
        Some(MarketOutcome::Up)
    } else if signal.signed_move_bps < 0.0 {
        Some(MarketOutcome::Down)
    } else {
        None
    }
}

fn desired_open_times(now_ts: u64, cadence_seconds: u64, prefetch_markets: usize) -> Vec<u64> {
    let current_open_ts = active_market_open_ts(now_ts, cadence_seconds);
    (0..=prefetch_markets)
        .map(|offset| current_open_ts + offset as u64 * cadence_seconds)
        .collect()
}

async fn resolve_tracked_markets(
    client: &reqwest::Client,
    params: &Btc5mParams,
    now_ts: u64,
) -> Result<Vec<TrackedMarket>, String> {
    let mut tracked_markets = Vec::new();
    for open_ts in desired_open_times(now_ts, params.cadence_seconds, params.prefetch_markets) {
        if let Some(market) =
            GammaBtc5mResolver::fetch_market(client, &params.market_slug_prefix, open_ts).await?
        {
            tracked_markets.push(TrackedMarket::new(market));
        }
    }

    if tracked_markets.is_empty() {
        Err("failed to resolve any btc_5m markets from gamma".to_string())
    } else {
        Ok(tracked_markets)
    }
}

fn tracked_market_assets(tracked_markets: &[TrackedMarket]) -> Vec<String> {
    let mut assets = Vec::new();
    for tracked in tracked_markets {
        push_unique_asset(&mut assets, tracked.market.up_token_id.clone());
        push_unique_asset(&mut assets, tracked.market.down_token_id.clone());
    }
    assets
}

fn push_unique_asset(assets: &mut Vec<String>, candidate: String) {
    if !assets.iter().any(|existing| existing == &candidate) {
        assets.push(candidate);
    }
}

fn next_cleanup_loss_total(current_total_usd: f64, cleanup_loss_usd: f64) -> f64 {
    current_total_usd + cleanup_loss_usd.max(0.0)
}

fn plan_shares_from_one_sided_budget(
    budget_usd: f64,
    ask_price: f64,
    ask_size: f64,
    min_order_size: f64,
) -> Option<f64> {
    if budget_usd <= 0.0 || ask_price <= 0.0 {
        return None;
    }

    let size = (budget_usd / ask_price).floor().min(ask_size.floor());
    if size < min_order_size {
        None
    } else {
        Some(size)
    }
}

fn plan_shares_from_pair_budget(
    budget_usd: f64,
    pair_sum: f64,
    first_ask_size: f64,
    second_ask_size: f64,
    min_order_size: f64,
) -> Option<f64> {
    if budget_usd <= 0.0 || pair_sum <= 0.0 {
        return None;
    }

    let size = (budget_usd / pair_sum)
        .floor()
        .min(first_ask_size.floor())
        .min(second_ask_size.floor());
    if size < min_order_size {
        None
    } else {
        Some(size)
    }
}

fn pair_sum_from_books(up_book: Option<&BookTop>, down_book: Option<&BookTop>) -> Option<f64> {
    Some(up_book?.ask_price? + down_book?.ask_price?)
}

fn book_for_outcome<'a>(
    up_book: Option<&'a BookTop>,
    down_book: Option<&'a BookTop>,
    outcome: MarketOutcome,
) -> Option<&'a BookTop> {
    match outcome {
        MarketOutcome::Up => up_book,
        MarketOutcome::Down => down_book,
    }
}

fn market_min_order_size(market: &Btc5mMarket) -> f64 {
    market
        .min_order_size
        .as_deref()
        .and_then(parse_f64)
        .unwrap_or(5.0)
}

fn response_fields(
    response: Option<&OrderResponse>,
) -> (Option<String>, Option<String>, String, String) {
    match response {
        Some(response) => (
            non_empty_string(&response.order_id),
            non_empty_string(&response.status),
            json_string(&response.transaction_hashes),
            json_string(&response.trade_ids),
        ),
        None => (None, None, "[]".to_string(), "[]".to_string()),
    }
}

fn json_string(values: &[String]) -> String {
    serde_json::to_string(values).unwrap_or_else(|_| "[]".to_string())
}

fn non_empty_string(value: &str) -> Option<String> {
    if value.trim().is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn outcome_label(outcome: MarketOutcome) -> &'static str {
    match outcome {
        MarketOutcome::Up => "up",
        MarketOutcome::Down => "down",
    }
}

fn mode_candidate_label(mode: ModeCandidate) -> &'static str {
    match mode {
        ModeCandidate::PairedNoSells => "paired_no_sells",
    }
}

fn first_leg_purpose_label(purpose: FirstLegPurpose) -> &'static str {
    match purpose {
        FirstLegPurpose::Probe => "probe",
        FirstLegPurpose::BurstPair => "burst_pair",
    }
}

fn cleanup_reason_label(reason: CleanupReason) -> &'static str {
    match reason {
        CleanupReason::BinanceStale => "binance_stale",
        CleanupReason::BinanceReversed => "binance_reversed",
        CleanupReason::PairUnavailable => "pair_unavailable",
        CleanupReason::EntryWindowExpired => "entry_window_expired",
        CleanupReason::OneSidedDisabled => "one_sided_disabled",
    }
}

fn seconds_since_open(market: &Btc5mMarket, now_ms: u64) -> u64 {
    (now_ms / 1000).saturating_sub(market.open_ts)
}

fn bps_change(from_price: f64, to_price: f64) -> f64 {
    if from_price <= 0.0 {
        0.0
    } else {
        ((to_price - from_price) / from_price) * 10_000.0
    }
}

async fn run_binance_feed(
    url: String,
    tx: broadcast::Sender<BinanceTrade>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), String> {
    let mut backoff_ms = 1_000_u64;

    loop {
        if *shutdown.borrow() {
            return Ok(());
        }

        let (ws_stream, _) = match connect_async(&url).await {
            Ok(result) => result,
            Err(error) => {
                tracing::warn!(error = %error, url = %url, "BTC 5m Binance feed connect failed");
                tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                backoff_ms = (backoff_ms * 2).min(60_000);
                continue;
            }
        };

        tracing::info!(url = %url, "BTC 5m Binance feed connected");
        backoff_ms = 1_000;

        let (mut write, mut read) = ws_stream.split();
        loop {
            tokio::select! {
                _ = shutdown.changed() => {
                    if *shutdown.borrow() {
                        let _ = write.send(Message::Close(None)).await;
                        return Ok(());
                    }
                }
                msg = read.next() => {
                    match msg {
                        Some(Ok(Message::Text(text))) => {
                            if let Some(trade) = parse_binance_trade_message(&text) {
                                let _ = tx.send(trade);
                            }
                        }
                        Some(Ok(Message::Ping(payload))) => {
                            write
                                .send(Message::Pong(payload))
                                .await
                                .map_err(|err| format!("failed to respond to Binance ping: {err}"))?;
                        }
                        Some(Ok(Message::Close(_))) => {
                            tracing::warn!("BTC 5m Binance feed closed by server");
                            break;
                        }
                        Some(Err(error)) => {
                            tracing::warn!(error = %error, "BTC 5m Binance feed stream error");
                            break;
                        }
                        None => {
                            tracing::warn!("BTC 5m Binance feed ended");
                            break;
                        }
                        _ => {}
                    }
                }
            }
        }

        if *shutdown.borrow() {
            return Ok(());
        }

        tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
        backoff_ms = (backoff_ms * 2).min(60_000);
    }
}

fn scalar_to_string(value: serde_json::Value) -> String {
    match value {
        serde_json::Value::String(value) => value,
        serde_json::Value::Number(value) => value.to_string(),
        other => other.to_string(),
    }
}

fn parse_f64(value: &str) -> Option<f64> {
    value.parse().ok()
}

fn format_decimal(value: f64) -> String {
    let rounded = value.round();
    if (value - rounded).abs() < 1e-9 {
        format!("{rounded:.0}")
    } else {
        let mut formatted = format!("{value:.6}");
        while formatted.contains('.') && formatted.ends_with('0') {
            formatted.pop();
        }
        if formatted.ends_with('.') {
            formatted.pop();
        }
        formatted
    }
}

fn book_top_from_snapshot(snapshot: &OrderBookSnapshot) -> BookTop {
    BookTop {
        bid_price: snapshot.best_bid.as_ref().and_then(|level| parse_f64(&level.price)),
        bid_size: snapshot.best_bid.as_ref().and_then(|level| parse_f64(&level.size)),
        ask_price: snapshot.best_ask.as_ref().and_then(|level| parse_f64(&level.price)),
        ask_size: snapshot.best_ask.as_ref().and_then(|level| parse_f64(&level.size)),
    }
}

fn now_ts() -> u64 {
    now_ms() / 1000
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

impl From<FirstLegPlan> for MarketActionPlan {
    fn from(value: FirstLegPlan) -> Self {
        MarketActionPlan::BuyFirstLeg(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_market_open_rounds_down_to_five_minute_boundary() {
        assert_eq!(active_market_open_ts(1_773_453_495, 300), 1_773_453_300);
        assert_eq!(active_market_open_ts(1_773_453_300, 300), 1_773_453_300);
    }

    #[test]
    fn tracked_market_open_times_include_current_and_next_window() {
        assert_eq!(
            desired_open_times(1_773_453_495, 300, 1),
            vec![1_773_453_300, 1_773_453_600]
        );
    }

    #[test]
    fn next_cleanup_loss_total_accumulates_only_positive_losses() {
        assert!((next_cleanup_loss_total(12.5, 1.25) - 13.75).abs() < 1e-9);
        assert!((next_cleanup_loss_total(12.5, -1.25) - 12.5).abs() < 1e-9);
    }

    #[test]
    fn parse_exact_event_response_maps_up_and_down_tokens_by_outcome_order() {
        let body = r#"
[
  {
    "slug": "btc-updown-5m-1773453300",
    "markets": [
      {
        "id": "1573226",
        "slug": "btc-updown-5m-1773453300",
        "outcomes": "[\"Up\", \"Down\"]",
        "clobTokenIds": "[\"11267214609570330352662364842538274243911779225806915548729968641032653733961\", \"80212860703476756903959300865032054498587343365417742399319648287028961909065\"]",
        "endDate": "2026-03-14T02:00:00Z",
        "orderPriceMinTickSize": 0.01,
        "orderMinSize": 5
      }
    ]
  }
]
"#;

        let market = GammaBtc5mResolver::parse_exact_event_response(
            body,
            "btc-updown-5m",
            1_773_453_300,
        )
        .unwrap()
        .expect("market");

        assert_eq!(market.slug, "btc-updown-5m-1773453300");
        assert_eq!(market.market_id, "1573226");
        assert_eq!(market.open_ts, 1_773_453_300);
        assert_eq!(market.close_ts, 1_773_453_600);
        assert_eq!(
            market.up_token_id,
            "11267214609570330352662364842538274243911779225806915548729968641032653733961"
        );
        assert_eq!(
            market.down_token_id,
            "80212860703476756903959300865032054498587343365417742399319648287028961909065"
        );
        assert_eq!(market.tick_size.as_deref(), Some("0.01"));
        assert_eq!(market.min_order_size.as_deref(), Some("5"));
    }

    #[test]
    fn parse_exact_event_response_returns_none_for_missing_slug() {
        let market = GammaBtc5mResolver::parse_exact_event_response(
            "[]",
            "btc-updown-5m",
            1_773_453_300,
        )
        .unwrap();

        assert!(market.is_none());
    }

    #[test]
    fn parse_binance_trade_message_extracts_price_and_trade_timestamp() {
        let trade = parse_binance_trade_message(
            r#"{"e":"aggTrade","E":1773453300123,"p":"84500.25","T":1773453300111}"#,
        )
        .expect("trade");

        assert!((trade.price - 84_500.25).abs() < 1e-9);
        assert_eq!(trade.timestamp_ms, 1_773_453_300_111);
    }

    #[test]
    fn compute_binance_signal_uses_open_and_recent_window() {
        let mut state = MarketExecutionState::default();
        let params = Btc5mParams::default();
        apply_binance_trade_to_state(
            &mut state,
            &BinanceTrade {
                price: 100_000.0,
                timestamp_ms: 1_000,
            },
            1_000,
            params.binance_buffer_window_ms,
        );
        apply_binance_trade_to_state(
            &mut state,
            &BinanceTrade {
                price: 100_010.0,
                timestamp_ms: 2_000,
            },
            1_000,
            params.binance_buffer_window_ms,
        );
        apply_binance_trade_to_state(
            &mut state,
            &BinanceTrade {
                price: 100_020.0,
                timestamp_ms: 4_000,
            },
            1_000,
            params.binance_buffer_window_ms,
        );

        let signal = compute_binance_signal(&state, &params, 4_500).expect("signal");
        assert!(signal.fresh);
        assert!((signal.signed_move_bps - 2.0).abs() < 1e-9);
        assert!(signal.recent_move_bps > 0.0);
    }

    #[test]
    fn plan_market_action_requires_fresh_binance_signal() {
        let market = sample_market();
        let params = Btc5mParams::default();
        let up_book = sample_book(0.49, 20.0);
        let down_book = sample_book(0.46, 20.0);

        assert!(plan_market_action(
            &market,
            &params,
            &MarketExecutionState::default(),
            (market.open_ts + 7) * 1000,
            Some(&up_book),
            Some(&down_book),
        )
        .is_none());
    }

    #[test]
    fn plan_market_action_probes_binance_aligned_side() {
        let market = sample_market();
        let params = Btc5mParams::default();
        let mut state = sample_state_with_binance(100_000.0, 100_020.0, (market.open_ts + 7) * 1000);
        state.last_attempt_at_ms = None;
        let up_book = sample_book(0.49, 20.0);
        let down_book = sample_book(0.46, 20.0);

        let plan = plan_market_action(
            &market,
            &params,
            &state,
            (market.open_ts + 7) * 1000,
            Some(&up_book),
            Some(&down_book),
        )
        .expect("plan");

        match plan {
            MarketActionPlan::BuyFirstLeg(plan) => {
                assert_eq!(plan.purpose, FirstLegPurpose::Probe);
                assert_eq!(plan.outcome, MarketOutcome::Up);
                assert_eq!(plan.size, "6");
                assert_eq!(plan.price, "0.49");
            }
            other => panic!("unexpected plan: {other:?}"),
        }
    }

    #[test]
    fn plan_market_action_pairs_pending_probe_when_second_leg_is_available() {
        let market = sample_market();
        let params = Btc5mParams::default();
        let mut state = sample_state_with_binance(100_000.0, 100_020.0, (market.open_ts + 8) * 1000);
        state.mode_candidate = Some(ModeCandidate::PairedNoSells);
        state.pending_first_leg = Some(PendingFirstLeg {
            outcome: MarketOutcome::Up,
            size: 6.0,
            entry_price: 0.49,
            entry_cost_usd: 2.94,
            purpose: FirstLegPurpose::Probe,
            created_at_ms: (market.open_ts + 7) * 1000,
        });
        let up_book = sample_book(0.49, 20.0);
        let down_book = sample_book(0.46, 20.0);

        let plan = plan_market_action(
            &market,
            &params,
            &state,
            (market.open_ts + 8) * 1000,
            Some(&up_book),
            Some(&down_book),
        )
        .expect("plan");

        match plan {
            MarketActionPlan::BuySecondLeg(plan) => {
                assert_eq!(plan.outcome, MarketOutcome::Down);
                assert_eq!(plan.size, "6");
                assert_eq!(plan.price, "0.46");
                assert!((plan.pair_sum - 0.95).abs() < 1e-9);
            }
            other => panic!("unexpected plan: {other:?}"),
        }
    }

    #[test]
    fn plan_market_action_enters_cleanup_when_one_sided_is_disabled_and_pair_cannot_form() {
        let market = sample_market();
        let mut params = Btc5mParams::default();
        params.cleanup_grace_ms = 500;
        let mut state = sample_state_with_binance(100_000.0, 100_018.0, (market.open_ts + 9) * 1000);
        state.mode_candidate = Some(ModeCandidate::PairedNoSells);
        state.pending_first_leg = Some(PendingFirstLeg {
            outcome: MarketOutcome::Up,
            size: 6.0,
            entry_price: 0.49,
            entry_cost_usd: 2.94,
            purpose: FirstLegPurpose::Probe,
            created_at_ms: (market.open_ts + 7) * 1000,
        });
        let up_book = BookTop {
            bid_price: Some(0.47),
            bid_size: Some(20.0),
            ask_price: Some(0.49),
            ask_size: Some(20.0),
        };
        let down_book = BookTop {
            bid_price: Some(0.50),
            bid_size: Some(20.0),
            ask_price: Some(0.60),
            ask_size: Some(2.0),
        };

        let plan = plan_market_action(
            &market,
            &params,
            &state,
            (market.open_ts + 9) * 1000,
            Some(&up_book),
            Some(&down_book),
        )
        .expect("plan");

        match plan {
            MarketActionPlan::Cleanup(plan) => {
                assert_eq!(plan.outcome, MarketOutcome::Up);
                assert_eq!(plan.reason, CleanupReason::OneSidedDisabled);
                assert_eq!(plan.size, "6");
                assert_eq!(plan.price, "0.47");
            }
            other => panic!("unexpected plan: {other:?}"),
        }
    }

    #[test]
    fn plan_market_action_enters_cleanup_on_binance_reversal() {
        let market = sample_market();
        let params = Btc5mParams::default();
        let mut state = sample_state_with_binance(100_000.0, 99_980.0, (market.open_ts + 8) * 1000);
        state.mode_candidate = Some(ModeCandidate::PairedNoSells);
        state.pending_first_leg = Some(PendingFirstLeg {
            outcome: MarketOutcome::Up,
            size: 6.0,
            entry_price: 0.49,
            entry_cost_usd: 2.94,
            purpose: FirstLegPurpose::Probe,
            created_at_ms: (market.open_ts + 7) * 1000,
        });
        let up_book = sample_book(0.49, 20.0);
        let down_book = sample_book(0.46, 20.0);

        let plan = plan_market_action(
            &market,
            &params,
            &state,
            (market.open_ts + 8) * 1000,
            Some(&up_book),
            Some(&down_book),
        )
        .expect("plan");

        match plan {
            MarketActionPlan::Cleanup(plan) => {
                assert_eq!(plan.reason, CleanupReason::BinanceReversed);
                assert_eq!(plan.outcome, MarketOutcome::Up);
            }
            other => panic!("unexpected plan: {other:?}"),
        }
    }

    fn sample_market() -> Btc5mMarket {
        Btc5mMarket {
            slug: "btc-updown-5m-1773453300".to_string(),
            market_id: "1573226".to_string(),
            open_ts: 1_773_453_300,
            close_ts: 1_773_453_600,
            up_token_id: "up-token".to_string(),
            down_token_id: "down-token".to_string(),
            min_order_size: Some("5".to_string()),
            tick_size: Some("0.01".to_string()),
        }
    }

    fn sample_book(ask_price: f64, ask_size: f64) -> BookTop {
        BookTop {
            bid_price: Some((ask_price - 0.02).max(0.0)),
            bid_size: Some(ask_size),
            ask_price: Some(ask_price),
            ask_size: Some(ask_size),
        }
    }

    fn sample_state_with_binance(
        open_price: f64,
        latest_price: f64,
        latest_trade_ts_ms: u64,
    ) -> MarketExecutionState {
        let mut state = MarketExecutionState::default();
        state.binance.open_price = Some(open_price);
        state.binance.latest_price = Some(latest_price);
        state.binance.latest_trade_ts_ms = Some(latest_trade_ts_ms);
        state.binance.recent_trades = VecDeque::from(vec![
            BinanceTrade {
                price: open_price,
                timestamp_ms: latest_trade_ts_ms.saturating_sub(2_000),
            },
            BinanceTrade {
                price: latest_price,
                timestamp_ms: latest_trade_ts_ms,
            },
        ]);
        state
    }
}
