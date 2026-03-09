#![allow(dead_code)]

use std::collections::BTreeMap;

use pm_strategy::quote::{DesiredQuote, DesiredQuotes, QuoteId};

use crate::order_state::WorkingQuote;

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
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReconciliationOutcome {
    pub commands: Vec<ExecutionCommand>,
    pub blocked: bool,
    pub blocked_quotes: Vec<QuoteId>,
}

pub struct LocalOrderManager {
    policy: ReconciliationPolicy,
}

impl Default for ReconciliationPolicy {
    fn default() -> Self {
        Self {
            min_price_change_units: 1,
            min_size_change_units: 1,
            replace_cooldown_ms: 0,
        }
    }
}

impl LocalOrderManager {
    pub fn new(policy: ReconciliationPolicy) -> Self {
        Self { policy }
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
        }
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

#[cfg(test)]
mod tests {
    use pm_strategy::quote::{DesiredQuote, DesiredQuotes, QuoteId};
    use rtt_core::trigger::{OrderType, Side};

    use super::*;
    use crate::order_state::{WorkingQuote, WorkingQuoteState};

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
        });
        let relaxed = LocalOrderManager::new(ReconciliationPolicy {
            min_price_change_units: 500,
            min_size_change_units: 1,
            replace_cooldown_ms: 0,
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
}
