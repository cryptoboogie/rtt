use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SourceId(String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    PolymarketWs,
    PolymarketRest,
    ExternalReference,
    ExternalTrade,
    Synthetic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstrumentKind {
    Source,
    Market,
    Asset,
    Symbol,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstrumentRef {
    pub source_id: SourceId,
    pub kind: InstrumentKind,
    pub instrument_id: String,
}

impl SourceId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for SourceId {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for SourceId {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl AsRef<str> for SourceId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for SourceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl InstrumentRef {
    pub fn asset(source_id: SourceId, instrument_id: impl Into<String>) -> Self {
        Self {
            source_id,
            kind: InstrumentKind::Asset,
            instrument_id: instrument_id.into(),
        }
    }

    pub fn market(source_id: SourceId, instrument_id: impl Into<String>) -> Self {
        Self {
            source_id,
            kind: InstrumentKind::Market,
            instrument_id: instrument_id.into(),
        }
    }

    pub fn symbol(source_id: SourceId, instrument_id: impl Into<String>) -> Self {
        Self {
            source_id,
            kind: InstrumentKind::Symbol,
            instrument_id: instrument_id.into(),
        }
    }

    pub fn source(source_id: SourceId) -> Self {
        Self {
            instrument_id: "_source".to_string(),
            source_id,
            kind: InstrumentKind::Source,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_and_instrument_identity_are_source_scoped() {
        let shared_id = "asset-1";
        let pm_source = SourceId::new("polymarket-public");
        let ref_source = SourceId::new("reference-mid");

        let polymarket_asset = InstrumentRef::asset(pm_source.clone(), shared_id);
        let external_symbol = InstrumentRef::symbol(ref_source.clone(), shared_id);

        assert_ne!(pm_source, ref_source);
        assert_ne!(polymarket_asset, external_symbol);
        assert_eq!(polymarket_asset.kind, InstrumentKind::Asset);
        assert_eq!(external_symbol.kind, InstrumentKind::Symbol);
    }
}
