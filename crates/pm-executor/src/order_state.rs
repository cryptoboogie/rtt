#![allow(dead_code)]

use pm_strategy::quote::{DesiredQuote, QuoteId};
use rtt_core::trigger::{OrderType, Side};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkingQuoteState {
    PendingSubmit,
    Working,
    PendingCancel,
    Canceled,
    Rejected { reason: String },
    UnknownOrStale { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExchangeObservedQuoteState {
    Working,
    PendingCancel,
    Canceled,
    Rejected { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExchangeObservedQuote {
    pub quote_id: QuoteId,
    pub client_order_id: Option<String>,
    pub state: ExchangeObservedQuoteState,
    pub observed_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkingQuote {
    pub quote_id: QuoteId,
    pub asset_id: String,
    pub side: Side,
    pub price: String,
    pub size: String,
    pub order_type: OrderType,
    pub state: WorkingQuoteState,
    pub client_order_id: Option<String>,
    pub created_at_ms: u64,
    pub last_update_ms: u64,
    pub last_command_ms: Option<u64>,
    pub last_exchange_observed_ms: Option<u64>,
}

impl WorkingQuote {
    pub fn pending_submit(desired: DesiredQuote, now_ms: u64) -> Self {
        Self {
            quote_id: desired.quote_id,
            asset_id: desired.asset_id,
            side: desired.side,
            price: desired.price,
            size: desired.size,
            order_type: desired.order_type,
            state: WorkingQuoteState::PendingSubmit,
            client_order_id: None,
            created_at_ms: now_ms,
            last_update_ms: now_ms,
            last_command_ms: Some(now_ms),
            last_exchange_observed_ms: None,
        }
    }

    pub fn mark_working(&mut self, client_order_id: impl Into<String>, now_ms: u64) {
        self.state = WorkingQuoteState::Working;
        self.client_order_id = Some(client_order_id.into());
        self.last_update_ms = now_ms;
    }

    pub fn mark_pending_cancel(&mut self, now_ms: u64) {
        self.state = WorkingQuoteState::PendingCancel;
        self.last_update_ms = now_ms;
        self.last_command_ms = Some(now_ms);
    }

    pub fn mark_canceled(&mut self, now_ms: u64) {
        self.state = WorkingQuoteState::Canceled;
        self.last_update_ms = now_ms;
    }

    pub fn mark_rejected(&mut self, reason: impl Into<String>, now_ms: u64) {
        self.state = WorkingQuoteState::Rejected {
            reason: reason.into(),
        };
        self.last_update_ms = now_ms;
    }

    pub fn mark_unknown_or_stale(&mut self, reason: impl Into<String>, now_ms: u64) {
        self.state = WorkingQuoteState::UnknownOrStale {
            reason: reason.into(),
        };
        self.last_update_ms = now_ms;
    }

    pub fn blocks_convergence(&self) -> bool {
        matches!(self.state, WorkingQuoteState::UnknownOrStale { .. })
    }

    pub fn is_cancelable(&self) -> bool {
        matches!(
            self.state,
            WorkingQuoteState::PendingSubmit
                | WorkingQuoteState::Working
                | WorkingQuoteState::PendingCancel
        )
    }

    pub fn mark_unknown_if_timed_out(
        &mut self,
        submit_timeout_ms: u64,
        cancel_timeout_ms: u64,
        now_ms: u64,
    ) -> bool {
        let Some(last_command_ms) = self.last_command_ms else {
            return false;
        };
        let elapsed_ms = now_ms.saturating_sub(last_command_ms);
        match self.state {
            WorkingQuoteState::PendingSubmit if elapsed_ms >= submit_timeout_ms => {
                self.mark_unknown_or_stale("submit_timeout", now_ms);
                true
            }
            WorkingQuoteState::PendingCancel if elapsed_ms >= cancel_timeout_ms => {
                self.mark_unknown_or_stale("cancel_timeout", now_ms);
                true
            }
            _ => false,
        }
    }

    pub fn mark_reconnect_stale(&mut self, now_ms: u64) {
        self.mark_unknown_or_stale("reconnect_resync_required", now_ms);
    }

    pub fn apply_exchange_observation(&mut self, observed: &ExchangeObservedQuote) {
        self.last_exchange_observed_ms = Some(observed.observed_at_ms);

        match (&self.state, &observed.state) {
            (_, ExchangeObservedQuoteState::Rejected { reason }) => {
                self.mark_rejected(reason.clone(), observed.observed_at_ms);
            }
            (_, ExchangeObservedQuoteState::Canceled) => {
                self.mark_canceled(observed.observed_at_ms);
            }
            (WorkingQuoteState::PendingCancel, ExchangeObservedQuoteState::Working) => {
                self.last_update_ms = observed.observed_at_ms;
            }
            (_, ExchangeObservedQuoteState::PendingCancel) => {
                self.state = WorkingQuoteState::PendingCancel;
                self.client_order_id = observed.client_order_id.clone();
                self.last_update_ms = observed.observed_at_ms;
            }
            (_, ExchangeObservedQuoteState::Working) => {
                if let Some(client_order_id) = observed.client_order_id.as_ref() {
                    self.mark_working(client_order_id.clone(), observed.observed_at_ms);
                } else {
                    self.state = WorkingQuoteState::Working;
                    self.last_update_ms = observed.observed_at_ms;
                }
            }
        }
    }

    pub fn apply_authoritative_absence(&mut self, now_ms: u64) -> bool {
        match self.state {
            WorkingQuoteState::PendingCancel
            | WorkingQuoteState::Canceled
            | WorkingQuoteState::UnknownOrStale { .. } => {
                self.mark_canceled(now_ms);
                false
            }
            WorkingQuoteState::PendingSubmit | WorkingQuoteState::Working => {
                self.mark_unknown_or_stale("exchange_missing_authoritative", now_ms);
                true
            }
            WorkingQuoteState::Rejected { .. } => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use pm_strategy::quote::{DesiredQuote, QuoteId};
    use rtt_core::trigger::{OrderType, Side};

    use super::*;

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

    #[test]
    fn working_quote_happy_path_transitions_are_explicit() {
        let desired = desired_quote("quote-1", "0.44", "10");
        let mut working = WorkingQuote::pending_submit(desired, 1_000);

        assert_eq!(working.state, WorkingQuoteState::PendingSubmit);

        working.mark_working("client-1", 1_100);
        assert_eq!(working.state, WorkingQuoteState::Working);
        assert_eq!(working.client_order_id.as_deref(), Some("client-1"));

        working.mark_pending_cancel(1_200);
        assert_eq!(working.state, WorkingQuoteState::PendingCancel);

        working.mark_canceled(1_300);
        assert_eq!(working.state, WorkingQuoteState::Canceled);
    }

    #[test]
    fn unknown_or_stale_is_a_first_class_state() {
        let desired = desired_quote("quote-1", "0.44", "10");
        let mut working = WorkingQuote::pending_submit(desired, 1_000);

        working.mark_unknown_or_stale("manual test hook", 1_050);

        assert_eq!(
            working.state,
            WorkingQuoteState::UnknownOrStale {
                reason: "manual test hook".to_string(),
            }
        );
        assert_eq!(working.last_update_ms, 1_050);
    }

    #[test]
    fn pending_submit_timeout_becomes_unknown_or_stale() {
        let desired = desired_quote("quote-1", "0.44", "10");
        let mut working = WorkingQuote::pending_submit(desired, 1_000);

        assert!(working.mark_unknown_if_timed_out(50, 100, 1_060));
        assert_eq!(
            working.state,
            WorkingQuoteState::UnknownOrStale {
                reason: "submit_timeout".to_string(),
            }
        );
    }

    #[test]
    fn pending_cancel_ignores_out_of_order_working_observation() {
        let desired = desired_quote("quote-1", "0.44", "10");
        let mut working = WorkingQuote::pending_submit(desired, 1_000);
        working.mark_working("client-1", 1_010);
        working.mark_pending_cancel(1_020);

        working.apply_exchange_observation(&ExchangeObservedQuote {
            quote_id: QuoteId::new("quote-1"),
            client_order_id: Some("client-1".to_string()),
            state: ExchangeObservedQuoteState::Working,
            observed_at_ms: 1_030,
        });

        assert_eq!(working.state, WorkingQuoteState::PendingCancel);

        working.apply_exchange_observation(&ExchangeObservedQuote {
            quote_id: QuoteId::new("quote-1"),
            client_order_id: Some("client-1".to_string()),
            state: ExchangeObservedQuoteState::Canceled,
            observed_at_ms: 1_040,
        });

        assert_eq!(working.state, WorkingQuoteState::Canceled);
    }
}
