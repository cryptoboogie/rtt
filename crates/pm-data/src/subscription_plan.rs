use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubscriptionOperation {
    Subscribe,
    Unsubscribe,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubscriptionAckPolicy {
    NoneExpected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubscriptionSemantics {
    pub supports_unsubscribe: bool,
    pub ack_policy: SubscriptionAckPolicy,
    pub reconnect_replays_desired_subscriptions: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubscriptionDiff {
    pub adds: Vec<String>,
    pub removes: Vec<String>,
    pub unchanged: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubscriptionCommand {
    pub operation: SubscriptionOperation,
    pub asset_ids: Vec<String>,
    pub shard_index: usize,
    pub pacing_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubscriptionPlannerConfig {
    pub max_batch_size: usize,
    pub pacing_ms: u64,
    pub shard_count: usize,
    pub shard_index: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubscriptionShard {
    pub shard_index: usize,
    pub asset_ids: Vec<String>,
}

impl Default for SubscriptionPlannerConfig {
    fn default() -> Self {
        Self {
            max_batch_size: 64,
            pacing_ms: 0,
            shard_count: 1,
            shard_index: 0,
        }
    }
}

pub fn market_subscription_semantics() -> SubscriptionSemantics {
    SubscriptionSemantics {
        supports_unsubscribe: true,
        ack_policy: SubscriptionAckPolicy::NoneExpected,
        reconnect_replays_desired_subscriptions: true,
    }
}

pub fn plan_subscription_diff(
    current: &BTreeSet<String>,
    desired: &BTreeSet<String>,
) -> SubscriptionDiff {
    SubscriptionDiff {
        adds: desired.difference(current).cloned().collect(),
        removes: current.difference(desired).cloned().collect(),
        unchanged: current.intersection(desired).cloned().collect(),
    }
}

pub fn assign_subscription_shards(
    desired: &BTreeSet<String>,
    shard_count: usize,
) -> Vec<SubscriptionShard> {
    let shard_count = shard_count.max(1);
    let mut shards = (0..shard_count)
        .map(|shard_index| SubscriptionShard {
            shard_index,
            asset_ids: Vec::new(),
        })
        .collect::<Vec<_>>();

    for (index, asset_id) in desired.iter().enumerate() {
        shards[index % shard_count].asset_ids.push(asset_id.clone());
    }

    shards
}

pub fn assigned_asset_ids_for_config(
    desired: &BTreeSet<String>,
    config: &SubscriptionPlannerConfig,
) -> Vec<String> {
    let shard_count = config.shard_count.max(1);
    let shard_index = config.shard_index.min(shard_count - 1);

    assign_subscription_shards(desired, shard_count)
        .into_iter()
        .find(|shard| shard.shard_index == shard_index)
        .map(|shard| shard.asset_ids)
        .unwrap_or_default()
}

pub fn plan_subscription_commands(
    current: &BTreeSet<String>,
    desired: &BTreeSet<String>,
    config: &SubscriptionPlannerConfig,
) -> Vec<SubscriptionCommand> {
    let shard_count = config.shard_count.max(1);
    let shard_index = config.shard_index.min(shard_count - 1);
    let max_batch_size = config.max_batch_size.max(1);

    let desired_for_shard = assigned_asset_ids_for_config(desired, config)
        .into_iter()
        .collect::<BTreeSet<_>>();

    let diff = plan_subscription_diff(current, &desired_for_shard);
    let mut commands = Vec::new();

    append_batched_commands(
        &mut commands,
        SubscriptionOperation::Unsubscribe,
        diff.removes,
        shard_index,
        max_batch_size,
        config.pacing_ms,
    );
    append_batched_commands(
        &mut commands,
        SubscriptionOperation::Subscribe,
        diff.adds,
        shard_index,
        max_batch_size,
        config.pacing_ms,
    );

    commands
}

fn append_batched_commands(
    commands: &mut Vec<SubscriptionCommand>,
    operation: SubscriptionOperation,
    asset_ids: Vec<String>,
    shard_index: usize,
    max_batch_size: usize,
    pacing_ms: u64,
) {
    for batch in asset_ids.chunks(max_batch_size) {
        commands.push(SubscriptionCommand {
            operation,
            asset_ids: batch.to_vec(),
            shard_index,
            pacing_ms: if commands.is_empty() { 0 } else { pacing_ms },
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(values: &[&str]) -> BTreeSet<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn market_subscription_semantics_capture_documented_behavior() {
        let semantics = market_subscription_semantics();

        assert!(semantics.supports_unsubscribe);
        assert_eq!(semantics.ack_policy, SubscriptionAckPolicy::NoneExpected);
        assert!(semantics.reconnect_replays_desired_subscriptions);
    }

    #[test]
    fn diff_planner_produces_adds_removes_and_unchanged_deterministically() {
        let diff = plan_subscription_diff(&set(&["asset-1", "asset-2"]), &set(&["asset-2", "asset-3"]));

        assert_eq!(diff.adds, vec!["asset-3".to_string()]);
        assert_eq!(diff.removes, vec!["asset-1".to_string()]);
        assert_eq!(diff.unchanged, vec!["asset-2".to_string()]);
    }

    #[test]
    fn command_planner_batches_large_changes_and_paces_follow_up_steps() {
        let commands = plan_subscription_commands(
            &set(&[]),
            &set(&["a1", "a2", "a3", "a4", "a5"]),
            &SubscriptionPlannerConfig {
                max_batch_size: 2,
                pacing_ms: 250,
                shard_count: 1,
                shard_index: 0,
            },
        );

        assert_eq!(commands.len(), 3);
        assert_eq!(commands[0].operation, SubscriptionOperation::Subscribe);
        assert_eq!(commands[0].asset_ids, vec!["a1".to_string(), "a2".to_string()]);
        assert_eq!(commands[0].pacing_ms, 0);
        assert_eq!(commands[1].asset_ids, vec!["a3".to_string(), "a4".to_string()]);
        assert_eq!(commands[1].pacing_ms, 250);
        assert_eq!(commands[2].asset_ids, vec!["a5".to_string()]);
        assert_eq!(commands[2].pacing_ms, 250);
    }

    #[test]
    fn shard_assignment_is_stable_and_explicit() {
        let first = assign_subscription_shards(&set(&["a1", "a2", "a3", "a4"]), 2);
        let second = assign_subscription_shards(&set(&["a1", "a2", "a3", "a4"]), 2);

        assert_eq!(first, second);
        assert_eq!(first[0].asset_ids, vec!["a1".to_string(), "a3".to_string()]);
        assert_eq!(first[1].asset_ids, vec!["a2".to_string(), "a4".to_string()]);
    }
}
