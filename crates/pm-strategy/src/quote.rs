use std::fmt;

use crate::types::{OrderType, Side};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct QuoteId(String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DesiredQuote {
    pub quote_id: QuoteId,
    pub asset_id: String,
    pub side: Side,
    pub price: String,
    pub size: String,
    pub order_type: OrderType,
    pub expiration_unix_secs: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct DesiredQuotes {
    pub quotes: Vec<DesiredQuote>,
}

impl DesiredQuotes {
    pub fn new(quotes: Vec<DesiredQuote>) -> Self {
        Self { quotes }
    }

    pub fn single(quote: DesiredQuote) -> Self {
        Self::new(vec![quote])
    }
}

impl QuoteId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for QuoteId {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for QuoteId {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl fmt::Display for QuoteId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl DesiredQuote {
    pub fn new(
        quote_id: QuoteId,
        asset_id: impl Into<String>,
        side: Side,
        price: impl Into<String>,
        size: impl Into<String>,
        order_type: OrderType,
    ) -> Self {
        Self {
            quote_id,
            asset_id: asset_id.into(),
            side,
            price: price.into(),
            size: size.into(),
            order_type,
            expiration_unix_secs: None,
        }
    }

    pub fn with_expiration(mut self, expiration_unix_secs: u64) -> Self {
        self.expiration_unix_secs = Some(expiration_unix_secs);
        self
    }
}
