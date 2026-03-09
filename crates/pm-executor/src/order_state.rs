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
}
