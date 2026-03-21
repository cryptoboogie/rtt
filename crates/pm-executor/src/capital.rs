use std::collections::BTreeMap;

use pm_strategy::strategy::InventoryPosition;

use crate::order_manager::ExecutionCommand;
use crate::order_state::WorkingQuote;

#[derive(Debug, Clone, PartialEq)]
pub struct DeploymentSnapshot {
    pub working_orders_usd: f64,
    pub unresolved_inventory_usd: f64,
    pub active_deployed_usd: f64,
    pub free_capital_usd: f64,
}

pub fn deployment_snapshot(
    budget_limit_usd: f64,
    working_quotes: &[WorkingQuote],
    inventory: &[InventoryPosition],
) -> DeploymentSnapshot {
    let working_orders_usd = working_orders_usd(working_quotes);
    let unresolved_inventory_usd = unresolved_inventory_usd(inventory);
    let active_deployed_usd = working_orders_usd + unresolved_inventory_usd;
    let free_capital_usd = (budget_limit_usd - active_deployed_usd).max(0.0);

    DeploymentSnapshot {
        working_orders_usd,
        unresolved_inventory_usd,
        active_deployed_usd,
        free_capital_usd,
    }
}

pub fn command_plan_within_budget(
    budget_limit_usd: f64,
    working_quotes: &[WorkingQuote],
    inventory: &[InventoryPosition],
    commands: &[ExecutionCommand],
) -> bool {
    projected_active_deployed_usd(budget_limit_usd, working_quotes, inventory, commands)
        .active_deployed_usd
        <= budget_limit_usd
}

pub fn projected_active_deployed_usd(
    budget_limit_usd: f64,
    working_quotes: &[WorkingQuote],
    inventory: &[InventoryPosition],
    commands: &[ExecutionCommand],
) -> DeploymentSnapshot {
    let mut projected_working_usd = working_orders_usd(working_quotes);
    let working_by_quote_id: BTreeMap<_, _> = working_quotes
        .iter()
        .map(|quote| (quote.quote_id.clone(), quote))
        .collect();

    for command in commands {
        match command {
            ExecutionCommand::Place(desired) => {
                projected_working_usd += quote_notional_usd(&desired.price, &desired.size);
            }
            ExecutionCommand::Cancel { quote_id } => {
                projected_working_usd -= working_by_quote_id
                    .get(quote_id)
                    .map(|quote| quote_notional_usd(&quote.price, &quote.size))
                    .unwrap_or_default();
            }
            ExecutionCommand::CancelAll => {
                projected_working_usd = 0.0;
            }
        }
    }

    let unresolved_inventory_usd = unresolved_inventory_usd(inventory);
    let active_deployed_usd = projected_working_usd.max(0.0) + unresolved_inventory_usd;
    let free_capital_usd = (budget_limit_usd - active_deployed_usd).max(0.0);

    DeploymentSnapshot {
        working_orders_usd: projected_working_usd.max(0.0),
        unresolved_inventory_usd,
        active_deployed_usd,
        free_capital_usd,
    }
}

fn working_orders_usd(working_quotes: &[WorkingQuote]) -> f64 {
    working_quotes
        .iter()
        .filter(|quote| quote.is_cancelable())
        .map(|quote| quote_notional_usd(&quote.price, &quote.size))
        .sum()
}

fn unresolved_inventory_usd(inventory: &[InventoryPosition]) -> f64 {
    inventory
        .iter()
        .map(|position| {
            parse_decimal(&position.net_notional)
                .unwrap_or_default()
                .abs()
        })
        .sum()
}

fn quote_notional_usd(price: &str, size: &str) -> f64 {
    parse_decimal(price).unwrap_or_default() * parse_decimal(size).unwrap_or_default()
}

fn parse_decimal(value: &str) -> Option<f64> {
    value.parse::<f64>().ok()
}

#[cfg(test)]
mod tests {
    use pm_strategy::quote::{DesiredQuote, QuoteId};
    use pm_strategy::types::{OrderType, Side};

    use super::*;

    fn working_quote(id: &str, price: &str, size: &str) -> WorkingQuote {
        WorkingQuote::pending_submit(
            DesiredQuote::new(
                QuoteId::new(id),
                "asset-1",
                Side::Buy,
                price,
                size,
                OrderType::GTD,
            ),
            1_000,
        )
    }

    #[test]
    fn deployment_snapshot_counts_working_and_inventory_capital() {
        let snapshot = deployment_snapshot(
            100.0,
            &[
                working_quote("quote-1", "0.45", "50"),
                working_quote("quote-2", "0.40", "50"),
            ],
            &[InventoryPosition {
                asset_id: "asset-1".to_string(),
                side: Side::Buy,
                filled_size: "50".to_string(),
                net_notional: "20".to_string(),
                updated_at_ms: 2_000,
            }],
        );

        assert!((snapshot.working_orders_usd - 42.5).abs() < 0.000_1);
        assert!((snapshot.unresolved_inventory_usd - 20.0).abs() < 0.000_1);
        assert!((snapshot.active_deployed_usd - 62.5).abs() < 0.000_1);
        assert!((snapshot.free_capital_usd - 37.5).abs() < 0.000_1);
    }

    #[test]
    fn projected_snapshot_applies_cancel_then_place_deltas() {
        let projected = projected_active_deployed_usd(
            100.0,
            &[working_quote("quote-1", "0.45", "50")],
            &[],
            &[
                ExecutionCommand::Cancel {
                    quote_id: QuoteId::new("quote-1"),
                },
                ExecutionCommand::Place(DesiredQuote::new(
                    QuoteId::new("quote-2"),
                    "asset-2",
                    Side::Buy,
                    "0.42",
                    "40",
                    OrderType::GTD,
                )),
            ],
        );

        assert!((projected.working_orders_usd - 16.8).abs() < 0.000_1);
        assert!((projected.active_deployed_usd - 16.8).abs() < 0.000_1);
        assert!((projected.free_capital_usd - 83.2).abs() < 0.000_1);
    }

    #[test]
    fn command_plan_rejects_budget_breach() {
        let allowed = command_plan_within_budget(
            100.0,
            &[working_quote("quote-1", "0.45", "50")],
            &[InventoryPosition {
                asset_id: "asset-1".to_string(),
                side: Side::Buy,
                filled_size: "50".to_string(),
                net_notional: "60".to_string(),
                updated_at_ms: 2_000,
            }],
            &[ExecutionCommand::Place(DesiredQuote::new(
                QuoteId::new("quote-2"),
                "asset-2",
                Side::Buy,
                "0.50",
                "50",
                OrderType::GTD,
            ))],
        );

        assert!(!allowed);
    }
}
