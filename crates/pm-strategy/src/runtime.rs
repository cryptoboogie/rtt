use std::collections::{BTreeMap, HashSet};

use tokio::sync::mpsc;

use crate::quote::DesiredQuotes;
use crate::strategy::{
    ExecutionMode, InventoryDelta, InventoryPosition, IsolationPolicy, QuoteStrategy,
    RequirementSelector, Strategy, StrategyDataRequirement, StrategyDataRequirementKind,
    StrategyRequirements, StrategyRuntimeView, TriggerStrategy,
};
use crate::types::{OrderBookSnapshot, TriggerMessage};
use rtt_core::{
    HotBookState, HotReferenceState, HotStateStore, HotStateView, InstrumentRef, SourceId,
    SourceKind, UpdateNotice,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProvisionedTopology {
    Shared,
    Dedicated,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvisionedInput {
    pub requirement: StrategyDataRequirement,
    pub source_id: SourceId,
    pub source_kind: SourceKind,
    pub subject: InstrumentRef,
    pub topology: ProvisionedTopology,
    pub instance_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeTopologyPlan {
    pub execution_mode: ExecutionMode,
    pub isolation_policy: IsolationPolicy,
    pub inputs: Vec<ProvisionedInput>,
}

pub struct SharedRuntimeScaffold {
    store: HotStateStore,
    topology: RuntimeTopologyPlan,
    seen_subjects: HashSet<String>,
    inventory: InventoryStore,
}

/// Consumes small notices and resolves the current runtime view from `HotStateStore`.
/// This preserves the existing snapshot-based strategy trait during the 12a migration.
pub struct NoticeDrivenRuntime {
    strategy: Box<dyn Strategy>,
    store: HotStateStore,
    notice_rx: mpsc::Receiver<UpdateNotice>,
    trigger_tx: mpsc::Sender<TriggerMessage>,
}

pub struct TriggerRuntime {
    strategy: Box<dyn TriggerStrategy>,
    scaffold: SharedRuntimeScaffold,
    notice_rx: mpsc::Receiver<UpdateNotice>,
    trigger_tx: mpsc::Sender<TriggerMessage>,
}

pub struct QuoteRuntime {
    strategy: Box<dyn QuoteStrategy>,
    scaffold: SharedRuntimeScaffold,
    notice_rx: mpsc::Receiver<UpdateNotice>,
    quote_tx: mpsc::Sender<DesiredQuotes>,
}

impl SharedRuntimeScaffold {
    pub fn new(store: HotStateStore, requirements: StrategyRequirements) -> Self {
        Self {
            store,
            topology: RuntimeTopologyPlan::from_requirements(&requirements),
            seen_subjects: HashSet::new(),
            inventory: InventoryStore::default(),
        }
    }

    pub fn topology(&self) -> &RuntimeTopologyPlan {
        &self.topology
    }

    pub fn resolve_view(&mut self, notice: &UpdateNotice) -> Option<StrategyRuntimeView> {
        if !self.is_relevant_notice(notice) {
            return None;
        }

        let resolved_notice = self.store.resolve_notice(notice);
        let mut books = Vec::new();
        let mut references = Vec::new();
        let mut inventory = Vec::new();

        for input in &self.topology.inputs {
            match input.requirement.kind {
                StrategyDataRequirementKind::PolymarketBbo
                | StrategyDataRequirementKind::PolymarketDepthTopN { .. }
                | StrategyDataRequirementKind::RewardMetadata => {
                    if let Some(book) = self.resolve_book(input, notice, resolved_notice.as_ref()) {
                        push_unique_book(&mut books, book);
                    }
                }
                StrategyDataRequirementKind::ExternalReferencePrice
                | StrategyDataRequirementKind::RecentTrades => {
                    if let Some(reference) =
                        self.resolve_reference(input, notice, resolved_notice.as_ref())
                    {
                        push_unique_reference(&mut references, reference);
                    }
                }
                StrategyDataRequirementKind::Inventory
                | StrategyDataRequirementKind::LiveOrderState => {
                    for position in self.resolve_inventory(input) {
                        push_unique_inventory(&mut inventory, position);
                    }
                }
            }
        }

        self.seen_subjects.insert(subject_key(&notice.subject));

        Some(StrategyRuntimeView::new(
            notice.clone(),
            books,
            references,
            inventory,
        ))
    }

    fn is_relevant_notice(&self, notice: &UpdateNotice) -> bool {
        self.topology
            .inputs
            .iter()
            .any(|input| input.subject == notice.subject)
    }

    fn resolve_book(
        &self,
        input: &ProvisionedInput,
        notice: &UpdateNotice,
        resolved_notice: Option<&HotStateView>,
    ) -> Option<HotBookState> {
        if input.subject == notice.subject {
            return match resolved_notice {
                Some(HotStateView::Book(book)) => Some(book.clone()),
                _ => None,
            };
        }

        if !self.seen_subjects.contains(&subject_key(&input.subject)) {
            return None;
        }

        self.store
            .book_state(&input.subject.source_id, &input.subject.instrument_id)
    }

    fn resolve_reference(
        &self,
        input: &ProvisionedInput,
        notice: &UpdateNotice,
        resolved_notice: Option<&HotStateView>,
    ) -> Option<HotReferenceState> {
        if input.subject == notice.subject {
            return match resolved_notice {
                Some(HotStateView::Reference(reference)) => Some(reference.clone()),
                _ => None,
            };
        }

        if !self.seen_subjects.contains(&subject_key(&input.subject)) {
            return None;
        }

        self.store.reference_state(&input.subject)
    }

    fn resolve_inventory(&self, input: &ProvisionedInput) -> Vec<InventoryPosition> {
        self.inventory.positions_for(input)
    }

    pub fn apply_inventory_delta(&mut self, delta: InventoryDelta) {
        self.inventory.apply_delta(delta);
    }

    pub fn inventory_positions(&self) -> Vec<InventoryPosition> {
        self.inventory.all_positions()
    }
}

impl RuntimeTopologyPlan {
    pub fn from_requirements(requirements: &StrategyRequirements) -> Self {
        let topology = match requirements.isolation_policy {
            IsolationPolicy::SharedFeedAcceptable => ProvisionedTopology::Shared,
            IsolationPolicy::DedicatedPreferred | IsolationPolicy::DedicatedRequired => {
                ProvisionedTopology::Dedicated
            }
        };

        let inputs = requirements
            .data
            .iter()
            .enumerate()
            .map(|(index, requirement)| {
                let (source_id, source_kind) = provision_source(requirement);
                let subject = provision_subject(requirement, source_id.clone());
                let instance_key = match topology {
                    ProvisionedTopology::Shared => format!("shared:{}", source_id.as_str()),
                    ProvisionedTopology::Dedicated => {
                        format!("dedicated:{}:{index}", source_id.as_str())
                    }
                };

                ProvisionedInput {
                    requirement: requirement.clone(),
                    source_id,
                    source_kind,
                    subject,
                    topology,
                    instance_key,
                }
            })
            .collect();

        Self {
            execution_mode: requirements.execution_mode,
            isolation_policy: requirements.isolation_policy,
            inputs,
        }
    }
}

impl NoticeDrivenRuntime {
    pub fn new(
        strategy: Box<dyn Strategy>,
        store: HotStateStore,
        notice_rx: mpsc::Receiver<UpdateNotice>,
        trigger_tx: mpsc::Sender<TriggerMessage>,
    ) -> Self {
        Self {
            strategy,
            store,
            notice_rx,
            trigger_tx,
        }
    }

    pub fn resolve_snapshot(&self, notice: &UpdateNotice) -> Option<OrderBookSnapshot> {
        self.store.project_snapshot(notice)
    }

    pub fn handle_notice(&mut self, notice: &UpdateNotice) -> Option<TriggerMessage> {
        let snapshot = self.resolve_snapshot(notice)?;
        self.strategy.on_book_update(&snapshot)
    }

    pub async fn run(&mut self) {
        while let Some(notice) = self.notice_rx.recv().await {
            let Some(trigger) = self.handle_notice(&notice) else {
                continue;
            };

            if self.trigger_tx.send(trigger).await.is_err() {
                break;
            }
        }
    }
}

impl TriggerRuntime {
    pub fn new(
        strategy: Box<dyn TriggerStrategy>,
        store: HotStateStore,
        notice_rx: mpsc::Receiver<UpdateNotice>,
        trigger_tx: mpsc::Sender<TriggerMessage>,
    ) -> Self {
        let requirements = strategy.requirements();
        let scaffold = SharedRuntimeScaffold::new(store, requirements);
        Self {
            strategy,
            scaffold,
            notice_rx,
            trigger_tx,
        }
    }

    pub fn topology(&self) -> &RuntimeTopologyPlan {
        self.scaffold.topology()
    }

    pub fn handle_notice(&mut self, notice: &UpdateNotice) -> Option<TriggerMessage> {
        let view = self.scaffold.resolve_view(notice)?;
        self.strategy.on_update(&view)
    }

    pub async fn run(&mut self) {
        while let Some(notice) = self.notice_rx.recv().await {
            let Some(trigger) = self.handle_notice(&notice) else {
                continue;
            };

            if self.trigger_tx.send(trigger).await.is_err() {
                break;
            }
        }
    }
}

impl QuoteRuntime {
    pub fn new(
        strategy: Box<dyn QuoteStrategy>,
        store: HotStateStore,
        notice_rx: mpsc::Receiver<UpdateNotice>,
        quote_tx: mpsc::Sender<DesiredQuotes>,
    ) -> Self {
        let requirements = strategy.requirements();
        let scaffold = SharedRuntimeScaffold::new(store, requirements);
        Self {
            strategy,
            scaffold,
            notice_rx,
            quote_tx,
        }
    }

    pub fn topology(&self) -> &RuntimeTopologyPlan {
        self.scaffold.topology()
    }

    pub fn apply_inventory_delta(&mut self, delta: InventoryDelta) {
        self.scaffold.apply_inventory_delta(delta);
    }

    pub fn inventory_positions(&self) -> Vec<InventoryPosition> {
        self.scaffold.inventory_positions()
    }

    pub fn handle_notice(&mut self, notice: &UpdateNotice) -> Option<DesiredQuotes> {
        let view = self.scaffold.resolve_view(notice)?;
        self.strategy.on_update(&view)
    }

    pub async fn run(&mut self) {
        while let Some(notice) = self.notice_rx.recv().await {
            let Some(desired_quotes) = self.handle_notice(&notice) else {
                continue;
            };

            if self.quote_tx.send(desired_quotes).await.is_err() {
                break;
            }
        }
    }
}

fn provision_source(requirement: &StrategyDataRequirement) -> (SourceId, SourceKind) {
    let source_id = match &requirement.selector {
        RequirementSelector::Source(source_id) => SourceId::new(source_id.clone()),
        RequirementSelector::Asset(_)
        | RequirementSelector::Market(_)
        | RequirementSelector::Symbol(_) => default_source_id(&requirement.kind),
    };

    (source_id, default_source_kind(&requirement.kind))
}

fn provision_subject(requirement: &StrategyDataRequirement, source_id: SourceId) -> InstrumentRef {
    match &requirement.selector {
        RequirementSelector::Asset(asset_id) => InstrumentRef::asset(source_id, asset_id.clone()),
        RequirementSelector::Market(market_id) => {
            InstrumentRef::market(source_id, market_id.clone())
        }
        RequirementSelector::Symbol(symbol) => InstrumentRef::symbol(source_id, symbol.clone()),
        RequirementSelector::Source(_) => InstrumentRef::source(source_id),
    }
}

fn default_source_id(kind: &StrategyDataRequirementKind) -> SourceId {
    match kind {
        StrategyDataRequirementKind::PolymarketBbo
        | StrategyDataRequirementKind::PolymarketDepthTopN { .. }
        | StrategyDataRequirementKind::RewardMetadata => SourceId::new("polymarket-public"),
        StrategyDataRequirementKind::ExternalReferencePrice => SourceId::new("external-reference"),
        StrategyDataRequirementKind::RecentTrades => SourceId::new("external-trades"),
        StrategyDataRequirementKind::Inventory | StrategyDataRequirementKind::LiveOrderState => {
            SourceId::new("strategy-runtime")
        }
    }
}

fn default_source_kind(kind: &StrategyDataRequirementKind) -> SourceKind {
    match kind {
        StrategyDataRequirementKind::PolymarketBbo
        | StrategyDataRequirementKind::PolymarketDepthTopN { .. }
        | StrategyDataRequirementKind::RewardMetadata => SourceKind::PolymarketWs,
        StrategyDataRequirementKind::ExternalReferencePrice => SourceKind::ExternalReference,
        StrategyDataRequirementKind::RecentTrades => SourceKind::ExternalTrade,
        StrategyDataRequirementKind::Inventory | StrategyDataRequirementKind::LiveOrderState => {
            SourceKind::Synthetic
        }
    }
}

fn push_unique_book(books: &mut Vec<HotBookState>, book: HotBookState) {
    if books.iter().any(|existing| {
        existing.notice.source_id == book.notice.source_id && existing.asset_id == book.asset_id
    }) {
        return;
    }

    books.push(book);
}

fn push_unique_reference(references: &mut Vec<HotReferenceState>, reference: HotReferenceState) {
    if references.iter().any(|existing| {
        existing.notice.source_id == reference.notice.source_id
            && existing.notice.subject.instrument_id == reference.notice.subject.instrument_id
    }) {
        return;
    }

    references.push(reference);
}

fn push_unique_inventory(inventory: &mut Vec<InventoryPosition>, position: InventoryPosition) {
    if inventory
        .iter()
        .any(|existing| existing.asset_id == position.asset_id && existing.side == position.side)
    {
        return;
    }

    inventory.push(position);
}

fn subject_key(subject: &InstrumentRef) -> String {
    format!(
        "{}:{:?}:{}",
        subject.source_id, subject.kind, subject.instrument_id
    )
}

#[derive(Default)]
struct InventoryStore {
    positions: BTreeMap<String, InventoryPosition>,
}

impl InventoryStore {
    fn apply_delta(&mut self, delta: InventoryDelta) {
        let key = inventory_key(&delta.asset_id, delta.side);
        let entry = self
            .positions
            .entry(key)
            .or_insert_with(|| InventoryPosition {
                asset_id: delta.asset_id.clone(),
                side: delta.side,
                filled_size: "0".to_string(),
                net_notional: "0".to_string(),
                updated_at_ms: delta.observed_at_ms,
            });

        entry.filled_size = format_units_trimmed(
            parse_units(&entry.filled_size).saturating_add(parse_units(&delta.filled_size_delta)),
        );
        entry.net_notional = format_units_trimmed(
            parse_units(&entry.net_notional).saturating_add(parse_units(&delta.notional_delta)),
        );
        entry.updated_at_ms = delta.observed_at_ms;
    }

    fn positions_for(&self, input: &ProvisionedInput) -> Vec<InventoryPosition> {
        self.positions
            .values()
            .filter(|position| match &input.requirement.selector {
                RequirementSelector::Asset(asset_id) => position.asset_id == *asset_id,
                RequirementSelector::Source(_) => true,
                RequirementSelector::Market(_) | RequirementSelector::Symbol(_) => false,
            })
            .cloned()
            .collect()
    }

    fn all_positions(&self) -> Vec<InventoryPosition> {
        self.positions.values().cloned().collect()
    }
}

fn inventory_key(asset_id: &str, side: crate::types::Side) -> String {
    format!("{asset_id}:{side:?}")
}

fn parse_units(value: &str) -> u64 {
    const SCALE: u64 = 1_000_000;

    let mut parts = value.split('.');
    let whole = parts.next().unwrap_or("0").parse::<u64>().unwrap_or(0);
    let frac = parts.next().unwrap_or("");
    let mut frac_buf = frac.as_bytes().to_vec();
    frac_buf.truncate(6);
    while frac_buf.len() < 6 {
        frac_buf.push(b'0');
    }

    let frac_units = std::str::from_utf8(&frac_buf)
        .ok()
        .and_then(|digits| digits.parse::<u64>().ok())
        .unwrap_or(0);

    whole.saturating_mul(SCALE).saturating_add(frac_units)
}

fn format_units(units: u64) -> String {
    let whole = units / 1_000_000;
    let frac = units % 1_000_000;
    format!("{whole}.{frac:06}")
}

fn format_units_trimmed(units: u64) -> String {
    let formatted = format_units(units);
    let trimmed = formatted.trim_end_matches('0').trim_end_matches('.');
    if trimmed.is_empty() {
        "0".to_string()
    } else {
        trimmed.to_string()
    }
}
