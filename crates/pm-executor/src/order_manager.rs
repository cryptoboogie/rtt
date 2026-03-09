#![allow(dead_code)]

use std::collections::BTreeMap;
use std::sync::Mutex;

use pm_strategy::quote::{DesiredQuote, DesiredQuotes, QuoteId};
use rtt_core::trigger::Side;

use crate::order_state::{ExchangeObservedQuote, WorkingQuote};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutionCommand {
    Place(DesiredQuote),
    Cancel { quote_id: QuoteId },
    CancelAll,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconciliationPolicy {
    pub min_price_change_units: u64,
    pub min_size_change_units: u64,
    pub replace_cooldown_ms: u64,
    pub submit_timeout_ms: u64,
    pub cancel_timeout_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReconciliationOutcome {
    pub commands: Vec<ExecutionCommand>,
    pub blocked: bool,
    pub blocked_quotes: Vec<QuoteId>,
    pub working: Vec<WorkingQuote>,
    pub resync_required: bool,
    pub exposure_deltas: Vec<ExposureDelta>,
}

pub struct LocalOrderManager {
    policy: ReconciliationPolicy,
    seen_fill_ids: Mutex<std::collections::BTreeSet<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExchangeFill {
    pub fill_id: String,
    pub asset_id: String,
    pub side: Side,
    pub filled_size: String,
    pub price: String,
    pub observed_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ExchangeSyncSnapshot {
    pub authoritative: bool,
    pub resync_pending: bool,
    pub quotes: Vec<ExchangeObservedQuote>,
    pub fills: Vec<ExchangeFill>,
    pub observed_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExposureDelta {
    pub asset_id: String,
    pub side: Side,
    pub filled_size_delta: String,
    pub notional_delta: String,
    pub observed_at_ms: u64,
}

pub trait ExchangeStateProvider {
    fn snapshot(&self) -> ExchangeSyncSnapshot;
}

impl Default for ReconciliationPolicy {
    fn default() -> Self {
        Self {
            min_price_change_units: 1,
            min_size_change_units: 1,
            replace_cooldown_ms: 0,
            submit_timeout_ms: 5_000,
            cancel_timeout_ms: 5_000,
        }
    }
}

impl LocalOrderManager {
    pub fn new(policy: ReconciliationPolicy) -> Self {
        Self {
            policy,
            seen_fill_ids: Mutex::new(Default::default()),
        }
    }

    pub fn reconcile(
        &self,
        desired: &DesiredQuotes,
        working: &[WorkingQuote],
        now_ms: u64,
    ) -> ReconciliationOutcome {
        let blocked_quotes: Vec<QuoteId> = working
            .iter()
            .filter(|quote| quote.blocks_convergence())
            .map(|quote| quote.quote_id.clone())
            .collect();
        if !blocked_quotes.is_empty() {
            return ReconciliationOutcome {
                commands: Vec::new(),
                blocked: true,
                blocked_quotes,
                working: working.to_vec(),
                resync_required: false,
                exposure_deltas: Vec::new(),
            };
        }

        let desired_by_id: BTreeMap<QuoteId, DesiredQuote> = desired
            .quotes
            .iter()
            .cloned()
            .map(|quote| (quote.quote_id.clone(), quote))
            .collect();
        let working_by_id: BTreeMap<QuoteId, &WorkingQuote> = working
            .iter()
            .map(|quote| (quote.quote_id.clone(), quote))
            .collect();

        if desired_by_id.is_empty() && working.iter().any(|quote| quote.is_cancelable()) {
            return ReconciliationOutcome {
                commands: vec![ExecutionCommand::CancelAll],
                blocked: false,
                blocked_quotes: Vec::new(),
                working: working.to_vec(),
                resync_required: false,
                exposure_deltas: Vec::new(),
            };
        }

        let mut commands = Vec::new();

        for (quote_id, quote) in &working_by_id {
            if !quote.is_cancelable() {
                continue;
            }

            match desired_by_id.get(quote_id) {
                None => commands.push(ExecutionCommand::Cancel {
                    quote_id: quote_id.clone(),
                }),
                Some(desired_quote)
                    if self.is_material_change(quote, desired_quote)
                        && self.cooldown_elapsed(quote, now_ms) =>
                {
                    commands.push(ExecutionCommand::Cancel {
                        quote_id: quote_id.clone(),
                    });
                }
                Some(_) => {}
            }
        }

        for (quote_id, desired_quote) in &desired_by_id {
            match working_by_id.get(quote_id) {
                None => commands.push(ExecutionCommand::Place(desired_quote.clone())),
                Some(working_quote) if !working_quote.is_cancelable() => {
                    commands.push(ExecutionCommand::Place(desired_quote.clone()));
                }
                Some(working_quote)
                    if self.is_material_change(working_quote, desired_quote)
                        && self.cooldown_elapsed(working_quote, now_ms) =>
                {
                    commands.push(ExecutionCommand::Place(desired_quote.clone()));
                }
                Some(_) => {}
            }
        }

        ReconciliationOutcome {
            commands,
            blocked: false,
            blocked_quotes: Vec::new(),
            working: working.to_vec(),
            resync_required: false,
            exposure_deltas: Vec::new(),
        }
    }

    pub fn reconcile_against_exchange<P: ExchangeStateProvider>(
        &self,
        desired: &DesiredQuotes,
        working: &[WorkingQuote],
        provider: &P,
        now_ms: u64,
    ) -> ReconciliationOutcome {
        let snapshot = provider.snapshot();
        self.reconcile_with_exchange(desired, working, &snapshot, now_ms)
    }

    pub fn reconcile_with_exchange(
        &self,
        desired: &DesiredQuotes,
        working: &[WorkingQuote],
        snapshot: &ExchangeSyncSnapshot,
        now_ms: u64,
    ) -> ReconciliationOutcome {
        let mut synchronized = working.to_vec();
        if snapshot.resync_pending {
            for quote in &mut synchronized {
                if quote.is_cancelable() {
                    quote.mark_reconnect_stale(snapshot.observed_at_ms);
                }
            }

            let blocked_quotes = synchronized
                .iter()
                .filter(|quote| quote.blocks_convergence())
                .map(|quote| quote.quote_id.clone())
                .collect();

            return ReconciliationOutcome {
                commands: Vec::new(),
                blocked: true,
                blocked_quotes,
                working: synchronized,
                resync_required: true,
                exposure_deltas: self.collect_new_exposure_deltas(snapshot),
            };
        }

        let mut observed_by_id: BTreeMap<QuoteId, &ExchangeObservedQuote> = snapshot
            .quotes
            .iter()
            .map(|quote| (quote.quote_id.clone(), quote))
            .collect();
        let mut resync_required = false;

        for quote in &mut synchronized {
            if quote.mark_unknown_if_timed_out(
                self.policy.submit_timeout_ms,
                self.policy.cancel_timeout_ms,
                now_ms,
            ) {
                resync_required = true;
            }

            if let Some(observed) = observed_by_id.remove(&quote.quote_id) {
                quote.apply_exchange_observation(observed);
            } else if snapshot.authoritative
                && quote.apply_authoritative_absence(snapshot.observed_at_ms)
            {
                resync_required = true;
            }
        }

        let mut outcome = self.reconcile(desired, &synchronized, now_ms);
        outcome.working = synchronized;
        outcome.resync_required = resync_required || outcome.blocked;
        outcome.exposure_deltas = self.collect_new_exposure_deltas(snapshot);
        outcome
    }

    fn cooldown_elapsed(&self, quote: &WorkingQuote, now_ms: u64) -> bool {
        quote
            .last_command_ms
            .map(|last| now_ms.saturating_sub(last) >= self.policy.replace_cooldown_ms)
            .unwrap_or(true)
    }

    fn is_material_change(&self, working: &WorkingQuote, desired: &DesiredQuote) -> bool {
        if working.asset_id != desired.asset_id
            || working.side != desired.side
            || working.order_type != desired.order_type
        {
            return true;
        }

        let price_delta = parse_units(&working.price).abs_diff(parse_units(&desired.price));
        let size_delta = parse_units(&working.size).abs_diff(parse_units(&desired.size));
        price_delta >= self.policy.min_price_change_units
            || size_delta >= self.policy.min_size_change_units
    }

    fn collect_new_exposure_deltas(&self, snapshot: &ExchangeSyncSnapshot) -> Vec<ExposureDelta> {
        let mut seen_fill_ids = self.seen_fill_ids.lock().unwrap();
        let mut deltas = Vec::new();
        for fill in &snapshot.fills {
            if !seen_fill_ids.insert(fill.fill_id.clone()) {
                continue;
            }

            let notional_units =
                parse_units(&fill.price) as u128 * parse_units(&fill.filled_size) as u128;
            deltas.push(ExposureDelta {
                asset_id: fill.asset_id.clone(),
                side: fill.side,
                filled_size_delta: format_units(parse_units(&fill.filled_size) as u128),
                notional_delta: format_units(notional_units / 1_000_000),
                observed_at_ms: fill.observed_at_ms,
            });
        }
        deltas
    }
}

fn parse_units(value: &str) -> u64 {
    const SCALE: u64 = 1_000_000;

    let mut parts = value.split('.');
    let whole = parts.next().unwrap_or("0").parse::<u64>().unwrap_or(0);
    let frac = parts.next().unwrap_or("");
    let mut frac_buf = frac.as_bytes().to_vec();
    frac_buf.truncate(6);
    while frac_buf.len() < 6 {
        frac_buf.push(b'0');
    }

    let frac_units = std::str::from_utf8(&frac_buf)
        .ok()
        .and_then(|digits| digits.parse::<u64>().ok())
        .unwrap_or(0);

    whole.saturating_mul(SCALE).saturating_add(frac_units)
}

fn format_units(units: u128) -> String {
    let whole = units / 1_000_000;
    let frac = units % 1_000_000;
    format!("{whole}.{frac:06}")
}

#[cfg(test)]
mod tests {
    use pm_strategy::quote::{DesiredQuote, DesiredQuotes, QuoteId};
    use rtt_core::trigger::{OrderType, Side};

    use super::*;
    use crate::order_state::{
        ExchangeObservedQuote, ExchangeObservedQuoteState, WorkingQuote, WorkingQuoteState,
    };

    #[derive(Clone)]
    struct FixtureExchangeStateProvider {
        snapshot: ExchangeSyncSnapshot,
    }

    impl ExchangeStateProvider for FixtureExchangeStateProvider {
        fn snapshot(&self) -> ExchangeSyncSnapshot {
            self.snapshot.clone()
        }
    }

    fn desired_quote(id: &str, price: &str, size: &str) -> DesiredQuote {
        DesiredQuote::new(
            QuoteId::new(id),
            "token_abc",
            Side::Buy,
            price,
            size,
            OrderType::GTC,
        )
    }

    fn working_quote(id: &str, price: &str, size: &str, last_command_ms: u64) -> WorkingQuote {
        let mut quote = WorkingQuote::pending_submit(desired_quote(id, price, size), 1_000);
        quote.mark_working(format!("client-{id}"), 1_100);
        quote.last_command_ms = Some(last_command_ms);
        quote
    }

    #[test]
    fn reconcile_places_missing_quotes_in_deterministic_order() {
        let desired = DesiredQuotes::new(vec![
            desired_quote("quote-b", "0.45", "10"),
            desired_quote("quote-a", "0.44", "10"),
        ]);

        let manager = LocalOrderManager::new(ReconciliationPolicy::default());
        let outcome = manager.reconcile(&desired, &[], 2_000);

        assert!(!outcome.blocked);
        assert_eq!(
            outcome.commands,
            vec![
                ExecutionCommand::Place(desired_quote("quote-a", "0.44", "10")),
                ExecutionCommand::Place(desired_quote("quote-b", "0.45", "10")),
            ]
        );
    }

    #[test]
    fn reconcile_cancels_stale_replaces_changed_and_noops_matching_quotes() {
        let desired = DesiredQuotes::new(vec![
            desired_quote("quote-1", "0.47", "10"),
            desired_quote("quote-2", "0.50", "10"),
        ]);
        let working = vec![
            working_quote("quote-1", "0.44", "10", 1_000),
            working_quote("quote-2", "0.50", "10", 1_000),
            working_quote("quote-3", "0.40", "10", 1_000),
        ];

        let manager = LocalOrderManager::new(ReconciliationPolicy::default());
        let outcome = manager.reconcile(&desired, &working, 2_000);

        assert_eq!(
            outcome.commands,
            vec![
                ExecutionCommand::Cancel {
                    quote_id: QuoteId::new("quote-1"),
                },
                ExecutionCommand::Cancel {
                    quote_id: QuoteId::new("quote-3"),
                },
                ExecutionCommand::Place(desired_quote("quote-1", "0.47", "10")),
            ]
        );
    }

    #[test]
    fn reconcile_uses_cancel_all_when_desired_is_empty() {
        let working = vec![
            working_quote("quote-1", "0.44", "10", 1_000),
            working_quote("quote-2", "0.45", "10", 1_000),
        ];

        let manager = LocalOrderManager::new(ReconciliationPolicy::default());
        let outcome = manager.reconcile(&DesiredQuotes::default(), &working, 2_000);

        assert_eq!(outcome.commands, vec![ExecutionCommand::CancelAll]);
    }

    #[test]
    fn unknown_or_stale_quotes_block_speculative_convergence() {
        let desired = DesiredQuotes::single(desired_quote("quote-1", "0.47", "10"));
        let working = vec![WorkingQuote {
            state: WorkingQuoteState::UnknownOrStale {
                reason: "injected".to_string(),
            },
            ..working_quote("quote-1", "0.44", "10", 1_000)
        }];

        let manager = LocalOrderManager::new(ReconciliationPolicy::default());
        let outcome = manager.reconcile(&desired, &working, 2_000);

        assert!(outcome.blocked);
        assert!(outcome.commands.is_empty());
        assert_eq!(outcome.blocked_quotes, vec![QuoteId::new("quote-1")]);
    }

    #[test]
    fn anti_thrash_cooldown_suppresses_rapid_replace_commands() {
        let desired = DesiredQuotes::single(desired_quote("quote-1", "0.49", "10"));
        let working = vec![working_quote("quote-1", "0.44", "10", 1_000)];
        let manager = LocalOrderManager::new(ReconciliationPolicy {
            min_price_change_units: 1,
            min_size_change_units: 1,
            replace_cooldown_ms: 500,
            ..ReconciliationPolicy::default()
        });

        let early = manager.reconcile(&desired, &working, 1_200);
        assert!(early.commands.is_empty());

        let late = manager.reconcile(&desired, &working, 1_600);
        assert_eq!(
            late.commands,
            vec![
                ExecutionCommand::Cancel {
                    quote_id: QuoteId::new("quote-1"),
                },
                ExecutionCommand::Place(desired_quote("quote-1", "0.49", "10")),
            ]
        );
    }

    #[test]
    fn per_instance_policy_is_isolated() {
        let desired = DesiredQuotes::single(desired_quote("quote-1", "0.4504", "10"));
        let working = vec![working_quote("quote-1", "0.4500", "10", 1_000)];

        let strict = LocalOrderManager::new(ReconciliationPolicy {
            min_price_change_units: 100,
            min_size_change_units: 1,
            replace_cooldown_ms: 0,
            ..ReconciliationPolicy::default()
        });
        let relaxed = LocalOrderManager::new(ReconciliationPolicy {
            min_price_change_units: 500,
            min_size_change_units: 1,
            replace_cooldown_ms: 0,
            ..ReconciliationPolicy::default()
        });

        assert!(!strict
            .reconcile(&desired, &working, 2_000)
            .commands
            .is_empty());
        assert!(relaxed
            .reconcile(&desired, &working, 2_000)
            .commands
            .is_empty());
    }

    #[test]
    fn authoritative_missing_quote_enters_unknown_then_resyncs_back_to_placeable_state() {
        let desired = DesiredQuotes::single(desired_quote("quote-1", "0.44", "10"));
        let working = vec![working_quote("quote-1", "0.44", "10", 1_000)];
        let provider = FixtureExchangeStateProvider {
            snapshot: ExchangeSyncSnapshot {
                authoritative: true,
                resync_pending: false,
                quotes: Vec::new(),
                fills: Vec::new(),
                observed_at_ms: 2_000,
            },
        };

        let manager = LocalOrderManager::new(ReconciliationPolicy::default());
        let first = manager.reconcile_against_exchange(&desired, &working, &provider, 2_000);
        assert!(first.blocked);
        assert!(first.resync_required);
        assert_eq!(
            first.working[0].state,
            WorkingQuoteState::UnknownOrStale {
                reason: "exchange_missing_authoritative".to_string(),
            }
        );

        let second = manager.reconcile_against_exchange(&desired, &first.working, &provider, 2_100);
        assert!(!second.blocked);
        assert_eq!(
            second.commands,
            vec![ExecutionCommand::Place(desired_quote(
                "quote-1", "0.44", "10"
            ))]
        );
        assert_eq!(second.working[0].state, WorkingQuoteState::Canceled);
    }

    #[test]
    fn out_of_order_exchange_observations_do_not_break_pending_cancel_state() {
        let desired = DesiredQuotes::single(desired_quote("quote-1", "0.44", "10"));
        let mut working = working_quote("quote-1", "0.44", "10", 1_000);
        working.mark_pending_cancel(1_200);
        let provider = FixtureExchangeStateProvider {
            snapshot: ExchangeSyncSnapshot {
                authoritative: true,
                resync_pending: false,
                quotes: vec![ExchangeObservedQuote {
                    quote_id: QuoteId::new("quote-1"),
                    client_order_id: Some("client-quote-1".to_string()),
                    state: ExchangeObservedQuoteState::Working,
                    observed_at_ms: 1_300,
                }],
                fills: Vec::new(),
                observed_at_ms: 1_300,
            },
        };

        let manager = LocalOrderManager::new(ReconciliationPolicy::default());
        let outcome = manager.reconcile_against_exchange(&desired, &[working], &provider, 1_300);

        assert!(!outcome.blocked);
        assert_eq!(outcome.working[0].state, WorkingQuoteState::PendingCancel);
        assert!(outcome.commands.is_empty());
    }

    #[test]
    fn exchange_fills_emit_exposure_deltas_only_once() {
        let desired = DesiredQuotes::default();
        let provider = FixtureExchangeStateProvider {
            snapshot: ExchangeSyncSnapshot {
                authoritative: true,
                resync_pending: false,
                quotes: Vec::new(),
                fills: vec![ExchangeFill {
                    fill_id: "fill-1".to_string(),
                    asset_id: "token_abc".to_string(),
                    side: Side::Buy,
                    filled_size: "10".to_string(),
                    price: "0.44".to_string(),
                    observed_at_ms: 2_000,
                }],
                observed_at_ms: 2_000,
            },
        };

        let manager = LocalOrderManager::new(ReconciliationPolicy::default());
        let first = manager.reconcile_against_exchange(&desired, &[], &provider, 2_000);
        assert_eq!(first.exposure_deltas.len(), 1);
        assert_eq!(first.exposure_deltas[0].filled_size_delta, "10.000000");
        assert_eq!(first.exposure_deltas[0].notional_delta, "4.400000");

        let second = manager.reconcile_against_exchange(&desired, &[], &provider, 2_100);
        assert!(second.exposure_deltas.is_empty());
    }

    #[test]
    fn reconnect_resync_pending_blocks_until_authoritative_snapshot_arrives() {
        let desired = DesiredQuotes::single(desired_quote("quote-1", "0.44", "10"));
        let working = vec![working_quote("quote-1", "0.44", "10", 1_000)];
        let reconnecting = FixtureExchangeStateProvider {
            snapshot: ExchangeSyncSnapshot {
                authoritative: false,
                resync_pending: true,
                quotes: Vec::new(),
                fills: Vec::new(),
                observed_at_ms: 1_500,
            },
        };

        let manager = LocalOrderManager::new(ReconciliationPolicy::default());
        let first = manager.reconcile_against_exchange(&desired, &working, &reconnecting, 1_500);
        assert!(first.blocked);
        assert!(first.resync_required);
        assert_eq!(
            first.working[0].state,
            WorkingQuoteState::UnknownOrStale {
                reason: "reconnect_resync_required".to_string(),
            }
        );

        let resynced = FixtureExchangeStateProvider {
            snapshot: ExchangeSyncSnapshot {
                authoritative: true,
                resync_pending: false,
                quotes: Vec::new(),
                fills: Vec::new(),
                observed_at_ms: 1_600,
            },
        };
        let second = manager.reconcile_against_exchange(&desired, &first.working, &resynced, 1_600);
        assert!(!second.blocked);
        assert_eq!(
            second.commands,
            vec![ExecutionCommand::Place(desired_quote(
                "quote-1", "0.44", "10"
            ))]
        );
    }
}
