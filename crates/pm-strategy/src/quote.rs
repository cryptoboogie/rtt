use crate::types::{OrderType, Side};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DesiredQuote {
    pub asset_id: String,
    pub side: Side,
    pub price: String,
    pub size: String,
    pub order_type: OrderType,
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
