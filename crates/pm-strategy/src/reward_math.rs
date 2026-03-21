use rtt_core::{HotBookLevel, HotStateValue};

pub const SINGLE_SIDED_SCALING_FACTOR: f64 = 3.0;

pub fn parse_decimal_to_units(value: &str) -> Option<u64> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.starts_with('-') {
        return None;
    }

    let mut parts = trimmed.split('.');
    let whole = parts.next()?.parse::<u64>().ok()?;
    let fraction = parts.next().unwrap_or("");
    if parts.next().is_some() {
        return None;
    }

    let mut padded = fraction.chars().take(6).collect::<String>();
    while padded.len() < 6 {
        padded.push('0');
    }
    let fraction_value = padded.parse::<u64>().ok()?;

    whole.checked_mul(1_000_000)?.checked_add(fraction_value)
}

pub fn size_cutoff_adjusted_midpoint(
    bids: &[HotBookLevel],
    asks: &[HotBookLevel],
    size_cutoff_units: u64,
) -> Option<HotStateValue> {
    let bid = cumulative_depth_price(bids, size_cutoff_units)?;
    let ask = cumulative_depth_price(asks, size_cutoff_units)?;
    let midpoint_units = (bid + ask) / 2;
    Some(HotStateValue {
        exact: format_decimal_from_units(midpoint_units),
        units: midpoint_units,
    })
}

pub fn qualifying_spread(midpoint_units: u64, price_units: u64) -> u64 {
    midpoint_units.abs_diff(price_units)
}

pub fn order_score(
    max_spread_units: u64,
    spread_units: u64,
    size_units: u64,
    min_size_units: u64,
    multiplier: f64,
) -> f64 {
    if max_spread_units == 0 || size_units < min_size_units || spread_units > max_spread_units {
        return 0.0;
    }

    let v = max_spread_units as f64;
    let s = spread_units as f64;
    let size = size_units as f64 / 1_000_000.0;
    ((v - s) / v).powi(2) * size * multiplier
}

pub fn q_min(q_one: f64, q_two: f64, midpoint_units: u64, single_sided_factor: f64) -> f64 {
    if midpoint_units >= 100_000 && midpoint_units <= 900_000 {
        q_one
            .min(q_two)
            .max((q_one / single_sided_factor).max(q_two / single_sided_factor))
    } else {
        q_one.min(q_two)
    }
}

pub fn round_down_to_tick(price_units: u64, tick_size_units: u64) -> u64 {
    if tick_size_units == 0 {
        return price_units;
    }
    price_units - (price_units % tick_size_units)
}

pub fn format_decimal_from_units(units: u64) -> String {
    let whole = units / 1_000_000;
    let fraction = units % 1_000_000;
    if fraction == 0 {
        return whole.to_string();
    }

    let mut rendered = format!("{whole}.{fraction:06}");
    while rendered.ends_with('0') {
        rendered.pop();
    }
    rendered
}

fn cumulative_depth_price(levels: &[HotBookLevel], size_cutoff_units: u64) -> Option<u64> {
    let mut cumulative = 0u64;
    let mut last_price = None;
    for level in levels {
        cumulative = cumulative.saturating_add(level.size.units);
        last_price = Some(level.price.units);
        if cumulative >= size_cutoff_units {
            return Some(level.price.units);
        }
    }

    last_price
}

#[cfg(test)]
mod tests {
    use super::*;

    fn level(price: &str, size: &str) -> HotBookLevel {
        HotBookLevel {
            price: HotStateValue {
                exact: price.to_string(),
                units: parse_decimal_to_units(price).unwrap_or_default(),
            },
            size: HotStateValue {
                exact: size.to_string(),
                units: parse_decimal_to_units(size).unwrap_or_default(),
            },
            price_ticks: None,
            size_lots: None,
        }
    }

    #[test]
    fn midpoint_uses_depth_cutoff_not_just_bbo() {
        let bids = vec![level("0.49", "20"), level("0.47", "40")];
        let asks = vec![level("0.51", "20"), level("0.55", "40")];
        let midpoint = size_cutoff_adjusted_midpoint(
            &bids,
            &asks,
            parse_decimal_to_units("50").unwrap_or_default(),
        )
        .expect("midpoint");

        assert_eq!(midpoint.exact, "0.51");
    }

    #[test]
    fn order_score_respects_max_spread_and_min_size() {
        let max_spread = parse_decimal_to_units("0.03").unwrap_or_default();
        let min_size = parse_decimal_to_units("50").unwrap_or_default();
        let size = parse_decimal_to_units("25").unwrap_or_default();
        let spread = parse_decimal_to_units("0.01").unwrap_or_default();

        assert_eq!(order_score(max_spread, spread, size, min_size, 1.0), 0.0);
        assert_eq!(
            order_score(
                max_spread,
                parse_decimal_to_units("0.04").unwrap_or_default(),
                parse_decimal_to_units("75").unwrap_or_default(),
                min_size,
                1.0,
            ),
            0.0
        );
    }

    #[test]
    fn q_min_applies_single_sided_scaling_inside_midpoint_band_only() {
        let inside_band = q_min(
            90.0,
            0.0,
            parse_decimal_to_units("0.50").unwrap_or_default(),
            SINGLE_SIDED_SCALING_FACTOR,
        );
        let outside_band = q_min(
            90.0,
            0.0,
            parse_decimal_to_units("0.95").unwrap_or_default(),
            SINGLE_SIDED_SCALING_FACTOR,
        );

        assert_eq!(inside_band, 30.0);
        assert_eq!(outside_band, 0.0);
    }
}
