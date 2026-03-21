use async_trait::async_trait;
use std::fmt;

use crate::snapshot::QuarantinedMarketRecord;
use rtt_core::{
    AssetId, MarketMeta, MarketStatus, MinOrderSize, OutcomeSide, OutcomeToken, Price,
    RewardFreshness, RewardParams, Size, TickSize,
};
use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryPageRequest {
    pub offset: usize,
    pub limit: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryPage {
    pub markets: Vec<MarketMeta>,
    pub quarantined: Vec<QuarantinedMarketRecord>,
    pub next_offset: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryProviderError {
    Transient(String),
    Permanent(String),
}

impl RegistryProviderError {
    pub fn transient(message: impl Into<String>) -> Self {
        Self::Transient(message.into())
    }

    pub fn permanent(message: impl Into<String>) -> Self {
        Self::Permanent(message.into())
    }

    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::Transient(_))
    }
}

impl fmt::Display for RegistryProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transient(message) | Self::Permanent(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for RegistryProviderError {}

#[async_trait]
pub trait RegistryProvider: Send + Sync {
    fn provider_name(&self) -> &str;

    async fn fetch_page(
        &self,
        request: RegistryPageRequest,
    ) -> Result<RegistryPage, RegistryProviderError>;
}

pub struct GammaRegistryProvider {
    provider_name: String,
    client: reqwest::Client,
    base_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrentRewardConfig {
    pub condition_id: String,
    pub reward: RewardParams,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawRewardToken {
    pub token_id: String,
    pub outcome: String,
    pub price: Price,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawRewardMarket {
    pub condition_id: String,
    pub market_competitiveness: Option<String>,
    pub tokens: Vec<RawRewardToken>,
}

pub struct PolymarketRewardProvider {
    client: reqwest::Client,
    base_url: String,
}

impl GammaRegistryProvider {
    pub fn new(provider_name: impl Into<String>) -> Self {
        Self::with_client_and_base_url(
            provider_name,
            reqwest::Client::new(),
            "https://gamma-api.polymarket.com",
        )
    }

    pub fn with_client_and_base_url(
        provider_name: impl Into<String>,
        client: reqwest::Client,
        base_url: impl Into<String>,
    ) -> Self {
        Self {
            provider_name: provider_name.into(),
            client,
            base_url: base_url.into(),
        }
    }

    pub fn parse_page(
        provider_name: &str,
        body: &str,
        request: &RegistryPageRequest,
    ) -> Result<RegistryPage, RegistryProviderError> {
        let events: Vec<GammaEvent> = serde_json::from_str(body).map_err(|err| {
            RegistryProviderError::permanent(format!("failed to parse gamma events: {err}"))
        })?;

        let mut markets = Vec::new();
        let mut quarantined = Vec::new();
        for event in events {
            for market in event.markets {
                match normalize_gamma_market(market) {
                    Ok(market) => markets.push(market),
                    Err(reason) => quarantined.push(QuarantinedMarketRecord {
                        record_id: reason.record_id,
                        reason: format!("{provider_name}: {}", reason.reason),
                    }),
                }
            }
        }

        let next_offset = if request.limit == 0 || markets.len() + quarantined.len() < request.limit
        {
            None
        } else {
            Some(request.offset + request.limit)
        };

        Ok(RegistryPage {
            markets,
            quarantined,
            next_offset,
        })
    }
}

impl PolymarketRewardProvider {
    pub fn new() -> Self {
        Self::with_client_and_base_url(reqwest::Client::new(), "https://clob.polymarket.com")
    }

    pub fn with_client_and_base_url(client: reqwest::Client, base_url: impl Into<String>) -> Self {
        Self {
            client,
            base_url: base_url.into(),
        }
    }

    pub async fn fetch_current_reward_configs(
        &self,
    ) -> Result<Vec<CurrentRewardConfig>, RegistryProviderError> {
        let response = self
            .client
            .get(format!(
                "{}/rewards/markets/current",
                self.base_url.trim_end_matches('/')
            ))
            .send()
            .await
            .map_err(|err| {
                RegistryProviderError::transient(format!("current rewards request failed: {err}"))
            })?;
        let status = response.status();
        let body = response.text().await.map_err(|err| {
            RegistryProviderError::transient(format!(
                "current rewards response body read failed: {err}"
            ))
        })?;

        if status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
            return Err(RegistryProviderError::transient(format!(
                "current rewards returned {status}"
            )));
        }
        if !status.is_success() {
            return Err(RegistryProviderError::permanent(format!(
                "current rewards returned {status}"
            )));
        }

        Self::parse_current_reward_configs(&body, now_ms())
    }

    pub async fn fetch_raw_market_rewards(
        &self,
        condition_id: &str,
    ) -> Result<Vec<RawRewardMarket>, RegistryProviderError> {
        let response = self
            .client
            .get(format!(
                "{}/rewards/markets/{}",
                self.base_url.trim_end_matches('/'),
                condition_id
            ))
            .send()
            .await
            .map_err(|err| {
                RegistryProviderError::transient(format!(
                    "raw market rewards request failed: {err}"
                ))
            })?;
        let status = response.status();
        let body = response.text().await.map_err(|err| {
            RegistryProviderError::transient(format!(
                "raw market rewards response body read failed: {err}"
            ))
        })?;

        if status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
            return Err(RegistryProviderError::transient(format!(
                "raw market rewards returned {status}"
            )));
        }
        if !status.is_success() {
            return Err(RegistryProviderError::permanent(format!(
                "raw market rewards returned {status}"
            )));
        }

        Self::parse_raw_market_rewards(&body)
    }

    pub fn parse_current_reward_configs(
        body: &str,
        fetched_at_ms: u64,
    ) -> Result<Vec<CurrentRewardConfig>, RegistryProviderError> {
        let envelope: CurrentRewardsEnvelope = serde_json::from_str(body).map_err(|err| {
            RegistryProviderError::permanent(format!(
                "failed to parse current rewards response: {err}"
            ))
        })?;

        envelope
            .data
            .into_iter()
            .map(|row| {
                let reward_max_spread = row
                    .rewards_max_spread
                    .map(ScalarField::into_string)
                    .map(|value| cents_to_price_string(&value))
                    .transpose()?
                    .map(Price::new);
                let reward_min_size = row
                    .rewards_min_size
                    .map(ScalarField::into_string)
                    .map(Size::new);
                let native_daily_rate = row
                    .native_daily_rate
                    .map(ScalarField::into_string)
                    .map(rtt_core::Notional::new);
                let sponsored_daily_rate = row
                    .sponsored_daily_rate
                    .map(ScalarField::into_string)
                    .map(rtt_core::Notional::new);
                let total_daily_rate = row
                    .total_daily_rate
                    .map(ScalarField::into_string)
                    .map(rtt_core::Notional::new);

                Ok(CurrentRewardConfig {
                    condition_id: row.condition_id,
                    reward: RewardParams {
                        rate_bps: None,
                        max_spread: reward_max_spread,
                        min_size: reward_min_size,
                        min_notional: None,
                        native_daily_rate,
                        sponsored_daily_rate,
                        total_daily_rate,
                        market_competitiveness: None,
                        fee_enabled: None,
                        updated_at_ms: Some(fetched_at_ms),
                        freshness: RewardFreshness::Fresh,
                    },
                })
            })
            .collect()
    }

    pub fn parse_raw_market_rewards(
        body: &str,
    ) -> Result<Vec<RawRewardMarket>, RegistryProviderError> {
        let envelope: RawRewardsEnvelope = serde_json::from_str(body).map_err(|err| {
            RegistryProviderError::permanent(format!(
                "failed to parse raw market rewards response: {err}"
            ))
        })?;

        Ok(envelope
            .data
            .into_iter()
            .map(|row| RawRewardMarket {
                condition_id: row.condition_id,
                market_competitiveness: row.market_competitiveness.map(|value| value.into_string()),
                tokens: row
                    .tokens
                    .into_iter()
                    .map(|token| RawRewardToken {
                        token_id: token.token_id,
                        outcome: token.outcome,
                        price: Price::new(token.price.into_string()),
                    })
                    .collect(),
            })
            .collect())
    }
}

#[async_trait]
impl RegistryProvider for GammaRegistryProvider {
    fn provider_name(&self) -> &str {
        &self.provider_name
    }

    async fn fetch_page(
        &self,
        request: RegistryPageRequest,
    ) -> Result<RegistryPage, RegistryProviderError> {
        let response = self
            .client
            .get(format!("{}/events", self.base_url.trim_end_matches('/')))
            .query(&[
                ("active", "true"),
                ("closed", "false"),
                ("limit", &request.limit.to_string()),
                ("offset", &request.offset.to_string()),
            ])
            .send()
            .await
            .map_err(|err| {
                RegistryProviderError::transient(format!("gamma request failed: {err}"))
            })?;

        let status = response.status();
        let body = response.text().await.map_err(|err| {
            RegistryProviderError::transient(format!("gamma response body read failed: {err}"))
        })?;

        if status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
            return Err(RegistryProviderError::transient(format!(
                "gamma returned {status}"
            )));
        }

        if !status.is_success() {
            return Err(RegistryProviderError::permanent(format!(
                "gamma returned {status}"
            )));
        }

        Self::parse_page(&self.provider_name, &body, &request)
    }
}

#[derive(Debug, Deserialize)]
struct GammaEvent {
    #[serde(default)]
    markets: Vec<GammaMarket>,
}

#[derive(Debug, Deserialize)]
struct CurrentRewardsEnvelope {
    #[serde(default)]
    data: Vec<CurrentRewardConfigRow>,
}

#[derive(Debug, Deserialize)]
struct CurrentRewardConfigRow {
    condition_id: String,
    rewards_max_spread: Option<ScalarField>,
    rewards_min_size: Option<ScalarField>,
    native_daily_rate: Option<ScalarField>,
    sponsored_daily_rate: Option<ScalarField>,
    total_daily_rate: Option<ScalarField>,
}

#[derive(Debug, Deserialize)]
struct RawRewardsEnvelope {
    #[serde(default)]
    data: Vec<RawRewardMarketRow>,
}

#[derive(Debug, Deserialize)]
struct RawRewardMarketRow {
    condition_id: String,
    market_competitiveness: Option<ScalarField>,
    #[serde(default)]
    tokens: Vec<RawRewardTokenRow>,
}

#[derive(Debug, Deserialize)]
struct RawRewardTokenRow {
    token_id: String,
    outcome: String,
    price: ScalarField,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GammaMarket {
    id: String,
    condition_id: Option<String>,
    #[serde(default)]
    neg_risk: bool,
    outcomes: Option<StringArrayField>,
    clob_token_ids: Option<StringArrayField>,
    active: bool,
    #[serde(default)]
    closed: bool,
    #[serde(default)]
    enable_order_book: bool,
    order_price_min_tick_size: Option<ScalarField>,
    tick_size: Option<ScalarField>,
    order_min_size: Option<ScalarField>,
    minimum_order_size: Option<ScalarField>,
    rewards_min_size: Option<ScalarField>,
    rewards_max_spread: Option<ScalarField>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum StringArrayField {
    Encoded(String),
    Plain(Vec<String>),
}

impl StringArrayField {
    fn into_vec(self) -> Result<Vec<String>, RegistryProviderError> {
        match self {
            Self::Encoded(raw) => serde_json::from_str(&raw).map_err(|err| {
                RegistryProviderError::permanent(format!("failed to parse encoded array: {err}"))
            }),
            Self::Plain(values) => Ok(values),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum ScalarField {
    String(String),
    Number(serde_json::Number),
}

impl ScalarField {
    fn into_string(self) -> String {
        match self {
            Self::String(value) => value,
            Self::Number(value) => value.to_string(),
        }
    }
}

struct QuarantineReason {
    record_id: Option<String>,
    reason: String,
}

fn normalize_gamma_market(market: GammaMarket) -> Result<MarketMeta, QuarantineReason> {
    let outcomes = market
        .outcomes
        .ok_or_else(|| quarantine(&market.id, "missing_outcomes"))?
        .into_vec()
        .map_err(|_| quarantine(&market.id, "invalid_outcomes"))?;
    let tokens = market
        .clob_token_ids
        .ok_or_else(|| quarantine(&market.id, "missing_clob_token_ids"))?
        .into_vec()
        .map_err(|_| quarantine(&market.id, "invalid_clob_token_ids"))?;

    if outcomes.len() != tokens.len() {
        return Err(quarantine(&market.id, "mismatched_outcomes_and_tokens"));
    }

    let mut yes_asset = None;
    let mut no_asset = None;
    for (outcome, token) in outcomes.into_iter().zip(tokens.into_iter()) {
        match outcome.to_ascii_lowercase().as_str() {
            "yes" => {
                yes_asset = Some(OutcomeToken::new(AssetId::new(token), OutcomeSide::Yes));
            }
            "no" => {
                no_asset = Some(OutcomeToken::new(AssetId::new(token), OutcomeSide::No));
            }
            _ => {}
        }
    }

    let (yes_asset, no_asset) = match (yes_asset, no_asset) {
        (Some(yes_asset), Some(no_asset)) => (yes_asset, no_asset),
        _ => return Err(quarantine(&market.id, "missing_yes_no_pair")),
    };

    let tick_size = market
        .order_price_min_tick_size
        .or(market.tick_size)
        .ok_or_else(|| quarantine(&market.id, "missing_tick_size"))?
        .into_string();
    let min_order_size = market
        .order_min_size
        .or(market.minimum_order_size)
        .map(ScalarField::into_string)
        .map(MinOrderSize::new);
    let reward_min_size = market
        .rewards_min_size
        .map(ScalarField::into_string)
        .map(Size::new);
    let reward_max_spread = market
        .rewards_max_spread
        .map(ScalarField::into_string)
        .map(|value| {
            cents_to_price_string(&value)
                .map_err(|_| quarantine(&market.id, "invalid_rewards_max_spread"))
        })
        .transpose()?
        .map(Price::new);
    let reward = if reward_min_size.is_some() || reward_max_spread.is_some() {
        Some(RewardParams {
            rate_bps: None,
            max_spread: reward_max_spread,
            min_size: reward_min_size,
            min_notional: None,
            native_daily_rate: None,
            sponsored_daily_rate: None,
            total_daily_rate: None,
            market_competitiveness: None,
            fee_enabled: None,
            updated_at_ms: None,
            freshness: RewardFreshness::Unknown,
        })
    } else {
        None
    };
    let status = if market.closed {
        MarketStatus::Closed
    } else if market.active && market.enable_order_book {
        MarketStatus::Active
    } else if market.active {
        MarketStatus::Suspended
    } else {
        MarketStatus::Unknown
    };

    Ok(MarketMeta {
        market_id: market.id.into(),
        yes_asset,
        no_asset,
        condition_id: market.condition_id,
        neg_risk: market.neg_risk,
        tick_size: TickSize::new(tick_size),
        min_order_size,
        status,
        reward,
    })
}

fn quarantine(record_id: &str, reason: &str) -> QuarantineReason {
    QuarantineReason {
        record_id: Some(record_id.to_string()),
        reason: reason.to_string(),
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn cents_to_price_string(raw: &str) -> Result<String, RegistryProviderError> {
    let cents = raw.parse::<f64>().map_err(|err| {
        RegistryProviderError::permanent(format!("invalid rewards max spread '{raw}': {err}"))
    })?;
    let price_units = cents / 100.0;
    let mut rendered = format!("{price_units:.6}");
    while rendered.contains('.') && rendered.ends_with('0') {
        rendered.pop();
    }
    if rendered.ends_with('.') {
        rendered.pop();
    }
    Ok(rendered)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rtt_core::{MarketStatus, OutcomeSide};

    #[test]
    fn gamma_page_normalizes_markets_and_quarantines_invalid_records() {
        let body = r#"
[
  {
    "id": "event-1",
    "markets": [
      {
        "id": "market-1",
        "conditionId": "condition-1",
        "negRisk": true,
        "outcomes": "[\"Yes\",\"No\"]",
        "clobTokenIds": "[\"token-yes\",\"token-no\"]",
        "active": true,
        "closed": false,
        "enableOrderBook": true,
        "orderPriceMinTickSize": 0.01,
        "orderMinSize": 5,
        "rewardsMinSize": 100,
        "rewardsMaxSpread": 3.5
      },
      {
        "id": "market-bad",
        "conditionId": "condition-bad",
        "outcomes": "[\"Yes\"]",
        "clobTokenIds": "[\"lonely-token\"]",
        "active": true,
        "closed": false,
        "enableOrderBook": true,
        "orderPriceMinTickSize": 0.01
      }
    ]
  }
]
"#;

        let page = GammaRegistryProvider::parse_page(
            "gamma-primary",
            body,
            &RegistryPageRequest {
                offset: 0,
                limit: 50,
            },
        )
        .unwrap();

        assert_eq!(page.markets.len(), 1);
        assert_eq!(page.quarantined.len(), 1);
        assert_eq!(page.next_offset, None);

        let market = &page.markets[0];
        assert_eq!(market.market_id.as_str(), "market-1");
        assert_eq!(market.status, MarketStatus::Active);
        assert!(market.neg_risk);
        assert_eq!(market.yes_asset.side, OutcomeSide::Yes);
        assert_eq!(market.yes_asset.asset_id.as_str(), "token-yes");
        assert_eq!(market.no_asset.asset_id.as_str(), "token-no");
        assert_eq!(market.tick_size.as_str(), "0.01");
        assert_eq!(
            market.min_order_size.as_ref().expect("min size").as_str(),
            "5"
        );
        assert!(market.reward.is_some());
        assert_eq!(page.quarantined[0].record_id.as_deref(), Some("market-bad"));
    }

    #[test]
    fn current_reward_configs_parse_into_enriched_reward_metadata() {
        let body = r#"
{
  "data": [
    {
      "condition_id": "condition-1",
      "rewards_max_spread": 4.5,
      "rewards_min_size": 50,
      "native_daily_rate": 5,
      "sponsored_daily_rate": 0.5,
      "total_daily_rate": 5.5
    }
  ]
}
"#;

        let parsed =
            PolymarketRewardProvider::parse_current_reward_configs(body, 1_700_000_000_000)
                .expect("reward configs");

        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].condition_id, "condition-1");
        assert_eq!(
            parsed[0].reward.max_spread.as_ref().unwrap().as_str(),
            "0.045"
        );
        assert_eq!(parsed[0].reward.min_size.as_ref().unwrap().as_str(), "50");
        assert_eq!(
            parsed[0].reward.total_daily_rate.as_ref().unwrap().as_str(),
            "5.5"
        );
        assert_eq!(parsed[0].reward.freshness, RewardFreshness::Fresh);
    }

    #[test]
    fn raw_reward_market_rows_capture_competitiveness_and_token_prices() {
        let body = r#"
{
  "data": [
    {
      "condition_id": "condition-1",
      "market_competitiveness": 13.40604,
      "tokens": [
        { "token_id": "yes-token", "outcome": "Yes", "price": 0.0245 },
        { "token_id": "no-token", "outcome": "No", "price": 0.9755 }
      ]
    }
  ]
}
"#;

        let parsed =
            PolymarketRewardProvider::parse_raw_market_rewards(body).expect("raw market rewards");

        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].condition_id, "condition-1");
        assert_eq!(
            parsed[0].market_competitiveness.as_deref(),
            Some("13.40604")
        );
        assert_eq!(parsed[0].tokens.len(), 2);
        assert_eq!(parsed[0].tokens[0].token_id, "yes-token");
        assert_eq!(parsed[0].tokens[0].price.as_str(), "0.0245");
    }
}
