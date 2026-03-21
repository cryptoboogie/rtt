use crate::quote::{DesiredQuote, DesiredQuotes, QuoteId};
use crate::reward_math::{
    format_decimal_from_units, parse_decimal_to_units, qualifying_spread, round_down_to_tick,
    size_cutoff_adjusted_midpoint,
};
use crate::strategy::{
    InventoryPosition, IsolationPolicy, QuoteStrategy, RequirementSelector,
    StrategyDataRequirement, StrategyDataRequirementKind, StrategyRequirements,
    StrategyRuntimeView,
};
use crate::types::{OrderType, Side};

#[derive(Debug, Clone)]
pub struct LiquidityRewardsMarket {
    pub condition_id: String,
    pub yes_asset_id: String,
    pub no_asset_id: String,
    pub tick_size: String,
    pub min_order_size: Option<String>,
    pub reward_max_spread: String,
    pub reward_min_size: String,
    pub end_time_ms: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct LiquidityRewardsParams {
    pub initial_bankroll_usd: f64,
    pub max_total_deployed_usd: f64,
    pub max_markets: usize,
    pub base_quote_size: f64,
    pub edge_buffer: f64,
    pub target_spread_cents: f64,
    pub quote_ttl_secs: u64,
    pub min_total_daily_rate: f64,
    pub max_market_competitiveness: f64,
    pub min_time_to_expiry_secs: u64,
    pub max_inventory_per_market: f64,
    pub max_unhedged_notional_per_market: f64,
}

pub struct LiquidityRewardsStrategy {
    #[allow(dead_code)]
    markets: Vec<LiquidityRewardsMarket>,
    #[allow(dead_code)]
    params: LiquidityRewardsParams,
}

impl LiquidityRewardsStrategy {
    pub fn new(markets: Vec<LiquidityRewardsMarket>, params: LiquidityRewardsParams) -> Self {
        Self { markets, params }
    }
}

impl QuoteStrategy for LiquidityRewardsStrategy {
    fn requirements(&self) -> StrategyRequirements {
        let mut data = Vec::with_capacity(self.markets.len() * 4);
        for market in &self.markets {
            data.push(StrategyDataRequirement {
                kind: StrategyDataRequirementKind::PolymarketDepthTopN { levels: 8 },
                selector: RequirementSelector::Asset(market.yes_asset_id.clone()),
            });
            data.push(StrategyDataRequirement {
                kind: StrategyDataRequirementKind::PolymarketDepthTopN { levels: 8 },
                selector: RequirementSelector::Asset(market.no_asset_id.clone()),
            });
            data.push(StrategyDataRequirement {
                kind: StrategyDataRequirementKind::Inventory,
                selector: RequirementSelector::Asset(market.yes_asset_id.clone()),
            });
            data.push(StrategyDataRequirement {
                kind: StrategyDataRequirementKind::Inventory,
                selector: RequirementSelector::Asset(market.no_asset_id.clone()),
            });
        }
        StrategyRequirements::quote(data, IsolationPolicy::SharedFeedAcceptable)
    }

    fn on_update(&mut self, view: &StrategyRuntimeView) -> Option<DesiredQuotes> {
        let mut remaining_budget = self.params.max_total_deployed_usd
            - active_inventory_notional_usd(view.inventory_positions());
        if remaining_budget <= 0.0 {
            return None;
        }

        let mut quotes = Vec::new();
        for market in &self.markets {
            let Some((mut planned, reserved_usd)) =
                self.plan_market_quotes(market, view, remaining_budget)
            else {
                continue;
            };
            remaining_budget -= reserved_usd;
            quotes.append(&mut planned);
            if quotes.len() >= self.params.max_markets * 2 {
                break;
            }
        }

        if quotes.is_empty() {
            None
        } else {
            Some(DesiredQuotes::new(quotes))
        }
    }

    fn name(&self) -> &str {
        "liquidity_rewards"
    }
}

impl LiquidityRewardsStrategy {
    fn plan_market_quotes(
        &self,
        market: &LiquidityRewardsMarket,
        view: &StrategyRuntimeView,
        remaining_budget_usd: f64,
    ) -> Option<(Vec<DesiredQuote>, f64)> {
        let yes_book = view.book(&market.yes_asset_id)?;
        let no_book = view.book(&market.no_asset_id)?;
        let now_ms = yes_book.timestamp_ms.max(no_book.timestamp_ms);

        if market
            .end_time_ms
            .map(|end_time_ms| {
                end_time_ms.saturating_sub(now_ms)
                    < self.params.min_time_to_expiry_secs.saturating_mul(1_000)
            })
            .unwrap_or(false)
        {
            return None;
        }

        if yes_book
            .reward
            .as_ref()
            .map(|reward| reward.freshness != rtt_core::RewardFreshness::Fresh)
            .unwrap_or(true)
            || no_book
                .reward
                .as_ref()
                .map(|reward| reward.freshness != rtt_core::RewardFreshness::Fresh)
                .unwrap_or(true)
        {
            return None;
        }

        let tick_size_units = parse_decimal_to_units(&market.tick_size)?;
        let min_size_units = parse_decimal_to_units(&market.reward_min_size)?;
        let market_min_size_units = market
            .min_order_size
            .as_deref()
            .and_then(parse_decimal_to_units)
            .unwrap_or(0);
        let quote_size_units = f64_to_units(self.params.base_quote_size)?
            .max(min_size_units)
            .max(market_min_size_units);
        let max_spread_units = parse_decimal_to_units(&market.reward_max_spread)?;

        let yes_midpoint =
            size_cutoff_adjusted_midpoint(&yes_book.bids, &yes_book.asks, min_size_units)?;
        let no_midpoint =
            size_cutoff_adjusted_midpoint(&no_book.bids, &no_book.asks, min_size_units)?;
        let yes_best_ask_units = yes_book.asks.first()?.price.units;
        let no_best_ask_units = no_book.asks.first()?.price.units;

        let yes_inventory = inventory_for(view.inventory_positions(), &market.yes_asset_id);
        let no_inventory = inventory_for(view.inventory_positions(), &market.no_asset_id);
        if exceeds_inventory_caps(yes_inventory.as_ref(), no_inventory.as_ref(), &self.params) {
            return None;
        }

        let ttl_expiration = (now_ms / 1_000)
            .saturating_add(60)
            .saturating_add(self.params.quote_ttl_secs);
        let target_offset_units =
            (f64_to_units(self.params.target_spread_cents / 100.0)? / 2).max(tick_size_units);
        let edge_buffer_units = f64_to_units(self.params.edge_buffer)?;
        let max_pair_total_units = 1_000_000u64.saturating_sub(edge_buffer_units);

        let yes_filled_size = yes_inventory
            .as_ref()
            .map(|position| parse_decimal_to_units(&position.filled_size).unwrap_or_default())
            .unwrap_or_default();
        let no_filled_size = no_inventory
            .as_ref()
            .map(|position| parse_decimal_to_units(&position.filled_size).unwrap_or_default())
            .unwrap_or_default();

        if yes_filled_size == no_filled_size {
            let (yes_bid_units, no_bid_units) = paired_bid_prices(
                yes_midpoint.units,
                no_midpoint.units,
                target_offset_units,
                tick_size_units,
                max_pair_total_units,
            )?;
            let yes_bid_units =
                clamp_passive_bid(yes_bid_units, tick_size_units, yes_best_ask_units)?;
            let no_bid_units = clamp_passive_bid(no_bid_units, tick_size_units, no_best_ask_units)?;
            if qualifying_spread(yes_midpoint.units, yes_bid_units) > max_spread_units
                || qualifying_spread(no_midpoint.units, no_bid_units) > max_spread_units
            {
                return None;
            }

            let reserved_usd = quote_notional_usd(yes_bid_units, quote_size_units)
                + quote_notional_usd(no_bid_units, quote_size_units);
            if reserved_usd > remaining_budget_usd {
                return None;
            }

            return Some((
                vec![
                    DesiredQuote::new(
                        QuoteId::new(format!("{}:yes:entry", market.condition_id)),
                        market.yes_asset_id.clone(),
                        Side::Buy,
                        format_decimal_from_units(yes_bid_units),
                        format_decimal_from_units(quote_size_units),
                        OrderType::GTD,
                    )
                    .with_expiration(ttl_expiration),
                    DesiredQuote::new(
                        QuoteId::new(format!("{}:no:entry", market.condition_id)),
                        market.no_asset_id.clone(),
                        Side::Buy,
                        format_decimal_from_units(no_bid_units),
                        format_decimal_from_units(quote_size_units),
                        OrderType::GTD,
                    )
                    .with_expiration(ttl_expiration),
                ],
                reserved_usd,
            ));
        }

        if yes_filled_size > no_filled_size {
            let completion_size_units = yes_filled_size.saturating_sub(no_filled_size);
            let max_completion_price_units =
                completion_price_cap(yes_inventory.as_ref(), edge_buffer_units)?;
            let completion_units = clamp_passive_bid(
                no_midpoint
                    .units
                    .saturating_sub(target_offset_units)
                    .min(max_completion_price_units),
                tick_size_units,
                no_best_ask_units,
            )?;
            let reserved_usd = quote_notional_usd(completion_units, completion_size_units);
            if reserved_usd > remaining_budget_usd {
                return None;
            }

            return Some((
                vec![DesiredQuote::new(
                    QuoteId::new(format!("{}:no:complete", market.condition_id)),
                    market.no_asset_id.clone(),
                    Side::Buy,
                    format_decimal_from_units(completion_units),
                    format_decimal_from_units(completion_size_units),
                    OrderType::GTD,
                )
                .with_expiration(ttl_expiration)],
                reserved_usd,
            ));
        }

        let completion_size_units = no_filled_size.saturating_sub(yes_filled_size);
        let max_completion_price_units =
            completion_price_cap(no_inventory.as_ref(), edge_buffer_units)?;
        let completion_units = clamp_passive_bid(
            yes_midpoint
                .units
                .saturating_sub(target_offset_units)
                .min(max_completion_price_units),
            tick_size_units,
            yes_best_ask_units,
        )?;
        let reserved_usd = quote_notional_usd(completion_units, completion_size_units);
        if reserved_usd > remaining_budget_usd {
            return None;
        }

        Some((
            vec![DesiredQuote::new(
                QuoteId::new(format!("{}:yes:complete", market.condition_id)),
                market.yes_asset_id.clone(),
                Side::Buy,
                format_decimal_from_units(completion_units),
                format_decimal_from_units(completion_size_units),
                OrderType::GTD,
            )
            .with_expiration(ttl_expiration)],
            reserved_usd,
        ))
    }
}

fn paired_bid_prices(
    yes_midpoint_units: u64,
    no_midpoint_units: u64,
    target_offset_units: u64,
    tick_size_units: u64,
    max_pair_total_units: u64,
) -> Option<(u64, u64)> {
    let mut yes_bid_units = round_down_to_tick(
        yes_midpoint_units.saturating_sub(target_offset_units),
        tick_size_units,
    );
    let mut no_bid_units = round_down_to_tick(
        no_midpoint_units.saturating_sub(target_offset_units),
        tick_size_units,
    );
    let total = yes_bid_units.saturating_add(no_bid_units);
    if total > max_pair_total_units {
        let excess = total - max_pair_total_units;
        let yes_cut = excess / 2;
        let no_cut = excess - yes_cut;
        yes_bid_units = round_down_to_tick(yes_bid_units.saturating_sub(yes_cut), tick_size_units);
        no_bid_units = round_down_to_tick(no_bid_units.saturating_sub(no_cut), tick_size_units);
    }
    if yes_bid_units == 0 || no_bid_units == 0 {
        return None;
    }
    Some((yes_bid_units, no_bid_units))
}

fn clamp_passive_bid(bid_units: u64, tick_size_units: u64, best_ask_units: u64) -> Option<u64> {
    let max_passive_units = best_ask_units.checked_sub(tick_size_units)?;
    let clamped = bid_units.min(max_passive_units);
    if clamped == 0 {
        None
    } else {
        Some(clamped)
    }
}

fn inventory_for(inventory: &[InventoryPosition], asset_id: &str) -> Option<InventoryPosition> {
    inventory
        .iter()
        .find(|position| position.asset_id == asset_id && position.side == Side::Buy)
        .cloned()
}

fn exceeds_inventory_caps(
    yes_inventory: Option<&InventoryPosition>,
    no_inventory: Option<&InventoryPosition>,
    params: &LiquidityRewardsParams,
) -> bool {
    let yes_filled = yes_inventory
        .and_then(|position| parse_decimal_to_units(&position.filled_size))
        .unwrap_or_default();
    let no_filled = no_inventory
        .and_then(|position| parse_decimal_to_units(&position.filled_size))
        .unwrap_or_default();
    let yes_notional = yes_inventory
        .and_then(|position| parse_decimal_to_units(&position.net_notional))
        .unwrap_or_default();
    let no_notional = no_inventory
        .and_then(|position| parse_decimal_to_units(&position.net_notional))
        .unwrap_or_default();

    let max_inventory_units = f64_to_units(params.max_inventory_per_market).unwrap_or_default();
    let max_notional_units =
        f64_to_units(params.max_unhedged_notional_per_market).unwrap_or_default();

    yes_filled.abs_diff(no_filled) > max_inventory_units
        || yes_notional.abs_diff(no_notional) > max_notional_units
}

fn completion_price_cap(
    inventory: Option<&InventoryPosition>,
    edge_buffer_units: u64,
) -> Option<u64> {
    let inventory = inventory?;
    let size_units = parse_decimal_to_units(&inventory.filled_size)?;
    if size_units == 0 {
        return None;
    }
    let notional_units = parse_decimal_to_units(&inventory.net_notional)?;
    let avg_price_units = notional_units.saturating_mul(1_000_000) / size_units;
    Some(
        1_000_000u64
            .saturating_sub(edge_buffer_units)
            .saturating_sub(avg_price_units),
    )
}

fn quote_notional_usd(price_units: u64, size_units: u64) -> f64 {
    (price_units as f64 / 1_000_000.0) * (size_units as f64 / 1_000_000.0)
}

fn active_inventory_notional_usd(inventory: &[InventoryPosition]) -> f64 {
    inventory
        .iter()
        .filter_map(|position| parse_decimal_to_units(&position.net_notional))
        .map(|units| units as f64 / 1_000_000.0)
        .sum()
}

fn f64_to_units(value: f64) -> Option<u64> {
    parse_decimal_to_units(&format!("{value:.6}"))
}

#[cfg(test)]
mod tests {
    use crate::quote::QuoteId;
    use crate::strategy::{InventoryPosition, StrategyRuntimeView};
    use crate::types::{OrderType, Side};
    use rtt_core::{
        feed_source::InstrumentRef, AssetId, HotBookLevel, HotBookState, HotStateValue, MarketId,
        RewardFreshness, RewardParams, SourceId, SourceKind, TickSize, UpdateKind, UpdateNotice,
    };

    use super::*;

    fn params() -> LiquidityRewardsParams {
        LiquidityRewardsParams {
            initial_bankroll_usd: 100.0,
            max_total_deployed_usd: 100.0,
            max_markets: 2,
            base_quote_size: 50.0,
            edge_buffer: 0.02,
            target_spread_cents: 2.0,
            quote_ttl_secs: 30,
            min_total_daily_rate: 1.0,
            max_market_competitiveness: 10.0,
            min_time_to_expiry_secs: 300,
            max_inventory_per_market: 100.0,
            max_unhedged_notional_per_market: 40.0,
        }
    }

    fn market() -> LiquidityRewardsMarket {
        LiquidityRewardsMarket {
            condition_id: "condition-1".to_string(),
            yes_asset_id: "yes-asset".to_string(),
            no_asset_id: "no-asset".to_string(),
            tick_size: "0.01".to_string(),
            min_order_size: Some("5".to_string()),
            reward_max_spread: "0.04".to_string(),
            reward_min_size: "50".to_string(),
            end_time_ms: Some(1_700_001_000_000),
        }
    }

    fn book_level(price: &str, size: &str) -> HotBookLevel {
        HotBookLevel {
            price: HotStateValue {
                exact: price.to_string(),
                units: crate::reward_math::parse_decimal_to_units(price).unwrap_or_default(),
            },
            size: HotStateValue {
                exact: size.to_string(),
                units: crate::reward_math::parse_decimal_to_units(size).unwrap_or_default(),
            },
            price_ticks: None,
            size_lots: None,
        }
    }

    fn book_state(asset_id: &str, bids: &[(&str, &str)], asks: &[(&str, &str)]) -> HotBookState {
        let source_id = SourceId::new("polymarket-public");
        let notice = UpdateNotice {
            source_id: source_id.clone(),
            source_kind: SourceKind::PolymarketWs,
            subject: InstrumentRef::asset(source_id, asset_id),
            kind: UpdateKind::BookSnapshot,
            version: 1,
            source_hash: Some("hash-1".to_string()),
        };
        let bids: Vec<_> = bids
            .iter()
            .map(|(price, size)| book_level(price, size))
            .collect();
        let asks: Vec<_> = asks
            .iter()
            .map(|(price, size)| book_level(price, size))
            .collect();
        HotBookState {
            notice,
            market_id: Some(MarketId::new("market-1")),
            condition_id: Some("condition-1".to_string()),
            asset_id: AssetId::new(asset_id),
            reward: Some(RewardParams {
                rate_bps: None,
                max_spread: Some(rtt_core::Price::new("0.04")),
                min_size: Some(rtt_core::Size::new("50")),
                min_notional: None,
                native_daily_rate: Some(rtt_core::Notional::new("5")),
                sponsored_daily_rate: None,
                total_daily_rate: Some(rtt_core::Notional::new("5")),
                market_competitiveness: Some("2".to_string()),
                fee_enabled: Some(false),
                updated_at_ms: Some(1_700_000_000_000),
                freshness: RewardFreshness::Fresh,
            }),
            bids: bids.clone(),
            asks: asks.clone(),
            best_bid: bids.first().cloned(),
            best_ask: asks.first().cloned(),
            midpoint: None,
            tick_size: Some(TickSize::new("0.01")),
            tick_size_units: Some(10_000),
            lot_size: None,
            lot_size_units: None,
            version: 1,
            timestamp_ms: 1_700_000_000_000,
            source_hash: Some("hash-1".to_string()),
        }
    }

    fn view(inventory: Vec<InventoryPosition>) -> StrategyRuntimeView {
        StrategyRuntimeView::new(
            UpdateNotice {
                source_id: SourceId::new("polymarket-public"),
                source_kind: SourceKind::PolymarketWs,
                subject: InstrumentRef::asset(SourceId::new("polymarket-public"), "yes-asset"),
                kind: UpdateKind::BookSnapshot,
                version: 1,
                source_hash: None,
            },
            vec![
                book_state(
                    "yes-asset",
                    &[("0.49", "20"), ("0.47", "40")],
                    &[("0.51", "20"), ("0.55", "40")],
                ),
                book_state(
                    "no-asset",
                    &[("0.48", "20"), ("0.46", "40")],
                    &[("0.52", "20"), ("0.56", "40")],
                ),
            ],
            Vec::new(),
            inventory,
        )
    }

    #[test]
    fn balanced_inventory_emits_paired_entry_quotes_with_stable_ids_and_gtd_expiry() {
        let mut strategy = LiquidityRewardsStrategy::new(vec![market()], params());

        let quotes = strategy.on_update(&view(Vec::new())).expect("quotes");

        assert_eq!(quotes.quotes.len(), 2);
        assert_eq!(
            quotes.quotes[0].quote_id,
            QuoteId::new("condition-1:yes:entry")
        );
        assert_eq!(
            quotes.quotes[1].quote_id,
            QuoteId::new("condition-1:no:entry")
        );
        assert!(quotes
            .quotes
            .iter()
            .all(|quote| quote.order_type == OrderType::GTD));
        assert!(quotes
            .quotes
            .iter()
            .all(|quote| quote.expiration_unix_secs.is_some()));

        let total_price: f64 = quotes
            .quotes
            .iter()
            .map(|quote| quote.price.parse::<f64>().unwrap())
            .sum();
        assert!(total_price <= 0.98);
    }

    #[test]
    fn one_sided_inventory_only_emits_completion_quote() {
        let mut strategy = LiquidityRewardsStrategy::new(vec![market()], params());
        let inventory = vec![InventoryPosition {
            asset_id: "yes-asset".to_string(),
            side: Side::Buy,
            filled_size: "50".to_string(),
            net_notional: "20".to_string(),
            updated_at_ms: 1_700_000_000_000,
        }];

        let quotes = strategy.on_update(&view(inventory)).expect("quotes");

        assert_eq!(quotes.quotes.len(), 1);
        assert_eq!(
            quotes.quotes[0].quote_id,
            QuoteId::new("condition-1:no:complete")
        );
        assert_eq!(quotes.quotes[0].side, Side::Buy);
    }

    #[test]
    fn inventory_caps_disable_market_when_unhedged_notional_is_too_large() {
        let mut strategy = LiquidityRewardsStrategy::new(vec![market()], params());
        let inventory = vec![InventoryPosition {
            asset_id: "yes-asset".to_string(),
            side: Side::Buy,
            filled_size: "50".to_string(),
            net_notional: "45".to_string(),
            updated_at_ms: 1_700_000_000_000,
        }];

        assert!(strategy.on_update(&view(inventory)).is_none());
    }

    #[test]
    fn balanced_inventory_clamps_bids_below_best_ask_to_keep_quotes_passive() {
        let mut strategy = LiquidityRewardsStrategy::new(vec![market()], params());
        let passive_view = StrategyRuntimeView::new(
            UpdateNotice {
                source_id: SourceId::new("polymarket-public"),
                source_kind: SourceKind::PolymarketWs,
                subject: InstrumentRef::asset(SourceId::new("polymarket-public"), "yes-asset"),
                kind: UpdateKind::BookSnapshot,
                version: 1,
                source_hash: None,
            },
            vec![
                book_state(
                    "yes-asset",
                    &[("0.50", "60")],
                    &[("0.51", "20"), ("0.55", "40")],
                ),
                book_state("no-asset", &[("0.45", "60")], &[("0.49", "60")]),
            ],
            Vec::new(),
            Vec::new(),
        );

        let quotes = strategy.on_update(&passive_view).expect("quotes");

        assert_eq!(quotes.quotes.len(), 2);
        assert_eq!(quotes.quotes[0].price, "0.5");
        assert_eq!(quotes.quotes[1].price, "0.46");
    }
}
