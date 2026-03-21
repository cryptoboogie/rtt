use std::fmt;

use serde::{Deserialize, Serialize};

macro_rules! string_value_type {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self::new(value)
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self::new(value)
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                self.as_str()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

string_value_type!(MarketId);
string_value_type!(AssetId);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeSide {
    Yes,
    No,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutcomeToken {
    pub asset_id: AssetId,
    pub side: OutcomeSide,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MarketStatus {
    Active,
    Closed,
    Suspended,
    Unknown,
}

string_value_type!(Price);
string_value_type!(Size);
string_value_type!(Notional);
string_value_type!(TickSize);
string_value_type!(MinOrderSize);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RewardFreshness {
    Fresh,
    StaleButUsable,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RewardParams {
    pub rate_bps: Option<u64>,
    pub max_spread: Option<Price>,
    pub min_size: Option<Size>,
    pub min_notional: Option<Notional>,
    pub native_daily_rate: Option<Notional>,
    pub sponsored_daily_rate: Option<Notional>,
    pub total_daily_rate: Option<Notional>,
    pub market_competitiveness: Option<String>,
    pub fee_enabled: Option<bool>,
    pub updated_at_ms: Option<u64>,
    pub freshness: RewardFreshness,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarketMeta {
    pub market_id: MarketId,
    pub yes_asset: OutcomeToken,
    pub no_asset: OutcomeToken,
    pub condition_id: Option<String>,
    #[serde(default)]
    pub neg_risk: bool,
    pub tick_size: TickSize,
    pub min_order_size: Option<MinOrderSize>,
    pub status: MarketStatus,
    pub reward: Option<RewardParams>,
}

impl OutcomeToken {
    pub fn new(asset_id: AssetId, side: OutcomeSide) -> Self {
        Self { asset_id, side }
    }
}

impl MarketMeta {
    pub fn asset_for_side(&self, side: OutcomeSide) -> &AssetId {
        match side {
            OutcomeSide::Yes => &self.yes_asset.asset_id,
            OutcomeSide::No => &self.no_asset.asset_id,
        }
    }

    pub fn side_for_asset(&self, asset_id: &AssetId) -> Option<OutcomeSide> {
        if self.yes_asset.asset_id == *asset_id {
            Some(OutcomeSide::Yes)
        } else if self.no_asset.asset_id == *asset_id {
            Some(OutcomeSide::No)
        } else {
            None
        }
    }

    pub fn is_tradable(&self) -> bool {
        self.status == MarketStatus::Active
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_market_meta() -> MarketMeta {
        MarketMeta {
            market_id: MarketId::new("market-1"),
            yes_asset: OutcomeToken::new(AssetId::new("yes-asset"), OutcomeSide::Yes),
            no_asset: OutcomeToken::new(AssetId::new("no-asset"), OutcomeSide::No),
            condition_id: Some("condition-1".to_string()),
            neg_risk: false,
            tick_size: TickSize::new("0.01"),
            min_order_size: Some(MinOrderSize::new("5")),
            status: MarketStatus::Active,
            reward: None,
        }
    }

    #[test]
    fn market_meta_tracks_yes_no_pairing_and_asset_lookup() {
        let meta = sample_market_meta();

        assert_eq!(
            meta.asset_for_side(OutcomeSide::Yes),
            &AssetId::new("yes-asset")
        );
        assert_eq!(
            meta.asset_for_side(OutcomeSide::No),
            &AssetId::new("no-asset")
        );
        assert_eq!(
            meta.side_for_asset(&AssetId::new("yes-asset")),
            Some(OutcomeSide::Yes)
        );
        assert_eq!(
            meta.side_for_asset(&AssetId::new("no-asset")),
            Some(OutcomeSide::No)
        );
        assert_eq!(meta.side_for_asset(&AssetId::new("other")), None);
    }

    #[test]
    fn exact_value_wrappers_serialize_without_losing_their_string_form() {
        #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
        struct NumericEnvelope {
            price: Price,
            size: Size,
            notional: Notional,
            tick_size: TickSize,
            min_order_size: MinOrderSize,
        }

        let envelope = NumericEnvelope {
            price: Price::new("0.4450"),
            size: Size::new("125.50"),
            notional: Notional::new("55.8475"),
            tick_size: TickSize::new("0.01"),
            min_order_size: MinOrderSize::new("5"),
        };

        let json = serde_json::to_string(&envelope).unwrap();
        let round_trip: NumericEnvelope = serde_json::from_str(&json).unwrap();

        assert_eq!(round_trip, envelope);
        assert!(json.contains("0.4450"));
        assert!(json.contains("125.50"));
    }

    #[test]
    fn market_meta_supports_generic_and_reward_enriched_markets() {
        let generic = sample_market_meta();
        assert!(generic.reward.is_none());
        assert!(generic.is_tradable());

        let reward_enriched = MarketMeta {
            reward: Some(RewardParams {
                rate_bps: Some(25),
                max_spread: Some(Price::new("0.02")),
                min_size: Some(Size::new("10")),
                min_notional: Some(Notional::new("100")),
                native_daily_rate: Some(Notional::new("5")),
                sponsored_daily_rate: Some(Notional::new("0.5")),
                total_daily_rate: Some(Notional::new("5.5")),
                market_competitiveness: Some("12.75".to_string()),
                fee_enabled: Some(true),
                updated_at_ms: Some(1_700_000_000_000),
                freshness: RewardFreshness::StaleButUsable,
            }),
            ..sample_market_meta()
        };

        assert_eq!(
            reward_enriched.reward.as_ref().unwrap().freshness,
            RewardFreshness::StaleButUsable
        );
        assert_eq!(
            reward_enriched
                .reward
                .as_ref()
                .unwrap()
                .total_daily_rate
                .as_ref()
                .unwrap()
                .as_str(),
            "5.5"
        );
        assert_eq!(
            reward_enriched
                .reward
                .as_ref()
                .unwrap()
                .market_competitiveness
                .as_deref(),
            Some("12.75")
        );
        assert_eq!(
            reward_enriched.reward.as_ref().unwrap().fee_enabled,
            Some(true)
        );
    }

    #[test]
    fn market_status_controls_tradable_flag() {
        let mut meta = sample_market_meta();
        assert!(meta.is_tradable());

        meta.status = MarketStatus::Suspended;
        assert!(!meta.is_tradable());

        meta.status = MarketStatus::Closed;
        assert!(!meta.is_tradable());
    }
}
