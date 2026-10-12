//! The tree cost ceiling (the RLM trio's second item): an opt-in cap on
//! what one session's whole spawn tree may spend. The scope is the tree
//! under the root session — children outlive the cell and the turn that
//! started them, so a per-cell or per-turn budget would reset while those
//! children keep spending.
//!
//! The ledger is host-side: the parent's own assistant usage and every
//! folded child attribution add into it, seeded from the session rows at
//! build. `rlm.spawn` refuses before the child starts when the USD ceiling
//! is reached, when the spawn-call limit is exhausted, or — under a USD
//! ceiling — when the requested model has no catalog price (an unpriced
//! model would never trip the ceiling). Parallel spawns in one cell all
//! pass the check together, so the overshoot is bounded only by the
//! children already running (the ruling's accepted bound).
//!
//! The ceilings and the admission count persist as a custom session row
//! ([`RLM_COST_CEILING_CUSTOM_TYPE`], latest row wins), so a worker restart
//! or reattach restores them instead of silently uncapping the tree.
//!
//! The scope reaches the whole tree: a spawn under a ceiling hands the
//! child the parent's remaining budget ([`InheritedCostCeiling`], carried on
//! the child's create as `runtimeMetadata.rlmCostCeiling`), and the child's
//! ledger enforces it as an upper bound beside its own ceiling — a child can
//! tighten its budget with `rlm.set_cost_ceiling`, never loosen it, and its
//! own children inherit what is left of it.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use pa_types::session::FileEntry;
use serde_json::{json, Value};

/// The custom-row type the ceilings and the admission count persist in.
pub(crate) const RLM_COST_CEILING_CUSTOM_TYPE: &str = "rlm_cost_ceiling";

/// The budget a child inherits from its parent's tree at spawn time: the
/// parent's remaining USD (its ceiling minus its spend) and its remaining
/// spawn admissions (the child's own admission already counted). `None`
/// dimensions are uncapped. The wire shape is the child create's
/// `runtimeMetadata.rlmCostCeiling` object (`costUsd`, `spawnLimit`).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InheritedCostCeiling {
    pub cost_usd: Option<f64>,
    pub spawn_limit: Option<u64>,
}

/// The durable-row mirror a host that owns persistence outside the session
/// manager (the daemon worker) installs: it receives each ceiling row's
/// `custom_message` wire shape.
pub type CeilingRowSink = Arc<dyn Fn(Value) + Send + Sync>;

/// One session tree's cost state: the ceilings (opt-in; `None` = that
/// dimension uncapped, today's behavior), the admitted-spawn count, and
/// the accumulated spend.
#[derive(Default)]
pub struct RlmTreeCost {
    ceiling_usd: Mutex<Option<f64>>,
    spawn_limit: Mutex<Option<u64>>,
    spawns_admitted: AtomicU64,
    spend_usd: Mutex<f64>,
    /// Whether the catalog prices the parent's own model: inherited-model
    /// spawns check this when no explicit selector was given. Unset reads
    /// as priced — no false refusals. A live model switch refreshes it.
    parent_model_priced: Mutex<Option<bool>>,
    /// The persistence owner's mirror for ceiling rows (`None` where the
    /// session manager owns the file).
    row_sink: Mutex<Option<CeilingRowSink>>,
    /// The budget inherited from the parent's tree (a spawned child's
    /// create carries it on every build); enforced beside the own ceilings.
    inherited: Mutex<Option<InheritedCostCeiling>>,
}

impl RlmTreeCost {
    /// Fresh state: no ceilings, nothing spent.
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Whether the catalog prices the parent's model (for inherited-model
    /// spawns). Defaults to priced until the engine states otherwise.
    #[must_use]
    ///
    /// # Panics
    ///
    /// Panics when the priced-flag mutex is poisoned.
    pub fn parent_model_priced(&self) -> bool {
        self.parent_model_priced
            .lock()
            .expect("tree parent priced lock")
            .unwrap_or(true)
    }

    /// Record the parent model's priced-ness: the engine resolves it from
    /// the catalog at build and again on every live model switch.
    ///
    /// # Panics
    ///
    /// Panics when the priced-flag mutex is poisoned.
    pub fn set_parent_model_priced(&self, priced: bool) {
        *self
            .parent_model_priced
            .lock()
            .expect("tree parent priced lock") = Some(priced);
    }

    /// Adopt the budget inherited from the parent's tree (a spawned child's
    /// create carries it): an upper bound the session's own ceilings can
    /// tighten but never loosen.
    ///
    /// # Panics
    ///
    /// Panics when the inherited-budget mutex is poisoned.
    pub fn inherit(&self, ceiling: InheritedCostCeiling) {
        *self.inherited.lock().expect("tree inherited lock") = Some(ceiling);
    }

    /// The budget a child spawned now inherits: per dimension, the tighter
    /// of this tree's own ceiling and its inherited one, minus what is
    /// already spent or admitted (the child's own admission included).
    /// `None` when no ceiling applies at all.
    ///
    /// # Panics
    ///
    /// Panics when a state mutex is poisoned.
    #[must_use]
    pub fn for_child(&self) -> Option<InheritedCostCeiling> {
        let inherited = *self.inherited.lock().expect("tree inherited lock");
        let own_cost = *self.ceiling_usd.lock().expect("tree cost ceiling lock");
        let own_limit = *self.spawn_limit.lock().expect("tree spawn limit lock");
        let cost = [own_cost, inherited.and_then(|ceiling| ceiling.cost_usd)]
            .into_iter()
            .flatten()
            .reduce(f64::min);
        let limit = [own_limit, inherited.and_then(|ceiling| ceiling.spawn_limit)]
            .into_iter()
            .flatten()
            .min();
        if cost.is_none() && limit.is_none() {
            return None;
        }
        let spent = *self.spend_usd.lock().expect("tree spend lock");
        // The child's own admission counts against the parent's allowance.
        let admitted = self.spawns_admitted.load(Ordering::Acquire) + 1;
        Some(InheritedCostCeiling {
            cost_usd: cost.map(|cost| (cost - spent).max(0.0)),
            spawn_limit: limit.map(|limit| limit.saturating_sub(admitted)),
        })
    }

    /// Install the persistence owner's mirror for ceiling rows.
    ///
    /// # Panics
    ///
    /// Panics when the sink mutex is poisoned.
    pub fn set_row_sink(&self, sink: CeilingRowSink) {
        *self.row_sink.lock().expect("tree row sink lock") = Some(sink);
    }

    /// Whether any ceiling is set (only then do admissions persist).
    ///
    /// # Panics
    ///
    /// Panics when either ceiling mutex is poisoned.
    #[must_use]
    pub fn has_ceiling(&self) -> bool {
        self.inherited
            .lock()
            .expect("tree inherited lock")
            .is_some()
            || self
                .ceiling_usd
                .lock()
                .expect("tree cost ceiling lock")
                .is_some()
            || self
                .spawn_limit
                .lock()
                .expect("tree spawn limit lock")
                .is_some()
    }

    /// The durable row's details: the ceilings and the admission count.
    ///
    /// # Panics
    ///
    /// Panics when either ceiling mutex is poisoned.
    #[must_use]
    pub(crate) fn row_details(&self) -> Value {
        json!({
            "version": 1,
            "cost_usd": *self.ceiling_usd.lock().expect("tree cost ceiling lock"),
            "spawn_limit": *self.spawn_limit.lock().expect("tree spawn limit lock"),
            "spawns_admitted": self.spawns_admitted.load(Ordering::Acquire),
        })
    }

    /// Persist the current ceilings and admission count: append the custom
    /// row to `session` (the in-process fold) and hand its wire shape to
    /// the persistence owner's mirror, if one is installed.
    ///
    /// # Errors
    ///
    /// Returns the session manager's append error.
    ///
    /// # Panics
    ///
    /// Panics when a state mutex is poisoned.
    pub(crate) fn persist(
        &self,
        session: &mut crate::session::manager::SessionManager,
    ) -> anyhow::Result<()> {
        let details = self.row_details();
        let content = format!("[rlm cost ceiling] {details}");
        session.append_custom_message(
            RLM_COST_CEILING_CUSTOM_TYPE,
            pa_types::ai::UserContent::Text(content.clone()),
            false,
            Some(details.clone()),
        )?;
        let sink = self.row_sink.lock().expect("tree row sink lock").clone();
        if let Some(sink) = sink {
            sink(json!({
                "customType": RLM_COST_CEILING_CUSTOM_TYPE,
                "content": content,
                "display": false,
                "details": details,
            }));
        }
        Ok(())
    }

    /// Restore the ceilings and the admission count from the session's
    /// latest decodable ceiling row (a malformed row is skipped, never a
    /// reason to uncap), and sum every recorded spend into the ledger:
    /// assistant usage and folded child attributions on every branch —
    /// money spent on an abandoned branch was still spent.
    ///
    /// # Panics
    ///
    /// Panics when a state mutex is poisoned.
    pub(crate) fn restore(&self, entries: &[FileEntry]) {
        let mut spend_usd = 0.0;
        let mut latest: Option<(Option<f64>, Option<u64>, u64)> = None;
        for entry in entries {
            match entry {
                FileEntry::Message {
                    message: pa_types::session::AgentMessage::Assistant(assistant),
                    ..
                } => spend_usd += assistant.usage.cost.total.as_f64(),
                FileEntry::ChildUsageAttributed { payload, .. } => {
                    spend_usd += payload.child_usage.cost.total.as_f64();
                }
                FileEntry::CustomMessage { payload, .. }
                    if payload.custom_type == RLM_COST_CEILING_CUSTOM_TYPE =>
                {
                    let details = payload.details.as_ref().unwrap_or(&Value::Null);
                    let decoded = (details.get("version").and_then(Value::as_u64) == Some(1))
                        .then(|| {
                            let cost = match details.get("cost_usd") {
                                None | Some(Value::Null) => Some(None),
                                Some(value) => value
                                    .as_f64()
                                    .filter(|cost| cost.is_finite() && *cost >= 0.0)
                                    .map(Some),
                            }?;
                            let limit = match details.get("spawn_limit") {
                                None | Some(Value::Null) => Some(None),
                                Some(value) => value.as_u64().filter(|limit| *limit >= 1).map(Some),
                            }?;
                            let admitted = details.get("spawns_admitted")?.as_u64()?;
                            Some((cost, limit, admitted))
                        })
                        .flatten();
                    if decoded.is_some() {
                        latest = decoded;
                    }
                }
                _ => {}
            }
        }
        self.note_spend(spend_usd);
        if let Some((cost, limit, admitted)) = latest {
            self.set(cost, limit);
            self.spawns_admitted.store(admitted, Ordering::Release);
        }
    }

    /// Set (or clear, with `None`) the ceilings. Overwrites the previous
    /// values wholesale: a parent may tighten or clear, never stack.
    ///
    /// # Panics
    ///
    /// Panics when either state mutex is poisoned.
    pub fn set(&self, ceiling_usd: Option<f64>, spawn_limit: Option<u64>) {
        *self.ceiling_usd.lock().expect("tree cost ceiling lock") = ceiling_usd;
        *self.spawn_limit.lock().expect("tree spawn limit lock") = spawn_limit;
    }

    /// Add `cost_usd` to the tree's spend (one assistant message's usage or
    /// one folded child attribution).
    ///
    /// # Panics
    ///
    /// Panics when the spend mutex is poisoned.
    pub fn note_spend(&self, cost_usd: f64) {
        if cost_usd <= 0.0 {
            return;
        }
        *self.spend_usd.lock().expect("tree spend lock") += cost_usd;
    }

    /// One spawn passed the gate and was admitted.
    pub fn note_spawn_admitted(&self) {
        self.spawns_admitted.fetch_add(1, Ordering::AcqRel);
    }

    /// The spawn gate. `model_priced` is `Some(false)` when the requested
    /// model has no catalog price (an unresolvable selector stays `None`
    /// and passes — the host rejects bad selectors on its own). Refusals
    /// are plain strings: they surface as the catchable `RuntimeError`
    /// `rlm.spawn` raises, before the child starts.
    ///
    /// # Errors
    ///
    /// Returns `Err` with the refusal reason when the ceilings refuse the
    /// spawn: the spawn limit is exhausted, the USD ceiling is reached, or
    /// (under a USD ceiling) the requested model has no catalog price.
    ///
    /// # Panics
    ///
    /// Panics when any state mutex is poisoned.
    pub fn check_spawn(&self, model_priced: Option<bool>) -> Result<(), String> {
        let inherited = *self.inherited.lock().expect("tree inherited lock");
        let scopes = [
            (
                "this session tree's",
                *self.ceiling_usd.lock().expect("tree cost ceiling lock"),
                *self.spawn_limit.lock().expect("tree spawn limit lock"),
            ),
            (
                "the parent tree's remaining",
                inherited.and_then(|ceiling| ceiling.cost_usd),
                inherited.and_then(|ceiling| ceiling.spawn_limit),
            ),
        ];
        let admitted = self.spawns_admitted.load(Ordering::Acquire);
        let spent = *self.spend_usd.lock().expect("tree spend lock");
        for (scope, ceiling, limit) in scopes {
            if let Some(limit) = limit {
                if admitted >= limit {
                    return Err(format!(
                        "rlm.spawn refused: {scope} spawn limit ({limit}) is exhausted"
                    ));
                }
            }
            if let Some(ceiling) = ceiling {
                if model_priced == Some(false) {
                    return Err(
                        "rlm.spawn refused: a cost ceiling is set, and the requested model has no \
                         catalog price, so its spend would never trip the ceiling"
                            .to_string(),
                    );
                }
                if spent >= ceiling {
                    return Err(format!(
                        "rlm.spawn refused: {scope} cost ceiling (${ceiling:.2}) is reached (${spent:.2} spent)"
                    ));
                }
            }
        }
        Ok(())
    }

    /// The ledger snapshot `rlm.set_cost_ceiling` returns.
    ///
    /// # Panics
    ///
    /// Panics when any state mutex is poisoned.
    #[must_use]
    pub fn snapshot(&self) -> Value {
        let ceiling = *self.ceiling_usd.lock().expect("tree cost ceiling lock");
        let spawn_limit = *self.spawn_limit.lock().expect("tree spawn limit lock");
        let spent = *self.spend_usd.lock().expect("tree spend lock");
        json!({
            // A number, rounded to micro-dollars (float sums carry noise).
            "cost_usd": (spent * 1e6).round() / 1e6,
            "spawns_admitted": self.spawns_admitted.load(Ordering::Acquire),
            "ceiling_usd": ceiling,
            "spawn_limit": spawn_limit,
            // The parent tree's budget at spawn (snake_case like the rest
            // of the snapshot; `null` for a root or an uncapped parent).
            "inherited": self.inherited.lock().expect("tree inherited lock").map(|ceiling| {
                json!({ "cost_usd": ceiling.cost_usd, "spawn_limit": ceiling.spawn_limit })
            }),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_uncapped_tree_admits_everything() {
        let tree = RlmTreeCost::new();
        assert_eq!(tree.check_spawn(None), Ok(()));
        assert_eq!(
            tree.check_spawn(Some(false)),
            Ok(()),
            "no ceiling set: pricing is moot"
        );
    }

    #[test]
    fn the_spawn_limit_refuses_at_the_cap() {
        let tree = RlmTreeCost::new();
        tree.set(None, Some(2));
        assert_eq!(tree.check_spawn(None), Ok(()));
        tree.note_spawn_admitted();
        tree.note_spawn_admitted();
        let refusal = tree.check_spawn(None).expect_err("the limit is exhausted");
        assert!(refusal.contains("spawn limit (2)"), "{refusal}");
    }

    #[test]
    fn the_usd_ceiling_refuses_at_the_spend() {
        let tree = RlmTreeCost::new();
        tree.set(Some(1.0), None);
        tree.note_spend(0.75);
        assert_eq!(tree.check_spawn(None), Ok(()));
        tree.note_spend(0.25);
        let refusal = tree.check_spawn(None).expect_err("the ceiling is reached");
        assert!(
            refusal.contains("$1.00") && refusal.contains("$1.00 spent"),
            "{refusal}"
        );
    }

    #[test]
    fn an_unpriced_model_is_refused_only_under_a_ceiling() {
        let tree = RlmTreeCost::new();
        assert_eq!(
            tree.check_spawn(Some(false)),
            Ok(()),
            "no ceiling: no pricing rule"
        );
        tree.set(Some(1.0), None);
        let refusal = tree
            .check_spawn(Some(false))
            .expect_err("unpriced under a ceiling");
        assert!(refusal.contains("no catalog price"), "{refusal}");
        // An unresolvable selector never false-refuses.
        assert_eq!(tree.check_spawn(None), Ok(()));
    }

    #[test]
    fn clearing_the_ceilings_restores_today() {
        let tree = RlmTreeCost::new();
        tree.set(Some(0.0), Some(0));
        assert!(tree.check_spawn(None).is_err());
        tree.set(None, None);
        assert_eq!(tree.check_spawn(None), Ok(()));
    }

    #[test]
    fn the_snapshot_reports_the_whole_ledger() {
        let tree = RlmTreeCost::new();
        tree.set(Some(2.5), Some(7));
        tree.note_spend(0.5);
        tree.note_spawn_admitted();
        assert_eq!(
            tree.snapshot(),
            json!({"cost_usd": 0.5, "spawns_admitted": 1, "ceiling_usd": 2.5, "spawn_limit": 7, "inherited": null})
        );
    }

    /// A custom row on the session (the persisted shape).
    fn ceiling_row(details: &Value) -> FileEntry {
        serde_json::from_value(json!({
            "type": "custom_message",
            "id": "c1",
            "parentId": null,
            "timestamp": "2026-10-12T00:00:00.000Z",
            "customType": RLM_COST_CEILING_CUSTOM_TYPE,
            "content": "[rlm cost ceiling]",
            "display": false,
            "details": details,
        }))
        .unwrap()
    }

    #[test]
    fn a_restart_restores_the_ceilings_and_the_admissions() {
        let before = RlmTreeCost::new();
        before.set(Some(2.0), Some(3));
        before.note_spawn_admitted();
        before.note_spawn_admitted();
        let entries = vec![
            ceiling_row(
                &json!({"version": 1, "cost_usd": 9.0, "spawn_limit": null, "spawns_admitted": 0}),
            ),
            ceiling_row(&before.row_details()),
            // A malformed later row never uncaps the tree.
            ceiling_row(&json!({"version": 1, "cost_usd": "lots"})),
        ];
        let after = RlmTreeCost::new();
        after.restore(&entries);
        assert_eq!(
            after.snapshot(),
            json!({"cost_usd": 0.0, "spawns_admitted": 2, "ceiling_usd": 2.0, "spawn_limit": 3, "inherited": null})
        );
        after.note_spawn_admitted();
        assert!(
            after.check_spawn(None).is_err(),
            "the restored limit counts the admissions made before the restart"
        );
    }

    #[test]
    fn the_restore_counts_spend_on_every_branch() {
        let assistant = |id: &str, parent: Option<&str>, cost: f64| -> FileEntry {
            serde_json::from_value(json!({
                "type": "message", "id": id, "parentId": parent,
                "timestamp": "2026-10-12T00:00:00.000Z",
                "message": {
                    "role": "assistant", "content": [], "api": "a", "provider": "p",
                    "model": "m", "stopReason": "stop", "timestamp": 0,
                    "usage": {"input": 1, "output": 1, "cacheRead": 0, "cacheWrite": 0,
                        "totalTokens": 2, "cost": {"input": 0, "output": 0,
                        "cacheRead": 0, "cacheWrite": 0, "total": cost}}
                }
            }))
            .unwrap()
        };
        // Two sibling branches under a1: both spent money.
        let entries = vec![
            assistant("a1", None, 0.25),
            assistant("a2", Some("a1"), 0.5),
            assistant("b2", Some("a1"), 0.75),
        ];
        let tree = RlmTreeCost::new();
        tree.restore(&entries);
        assert_eq!(tree.snapshot()["cost_usd"], json!(1.5));
    }

    #[test]
    fn a_child_inherits_the_remaining_budget_and_cannot_loosen_it() {
        let root = RlmTreeCost::new();
        assert_eq!(
            root.for_child(),
            None,
            "an uncapped tree hands down nothing"
        );
        root.set(Some(5.0), Some(3));
        root.note_spend(1.5);
        root.note_spawn_admitted();
        // The next child's own admission counts: 3 - (1 + 1) = 1 left.
        let inherited = root.for_child().expect("a capped tree hands down a budget");
        assert_eq!(
            inherited,
            InheritedCostCeiling {
                cost_usd: Some(3.5),
                spawn_limit: Some(1),
            }
        );
        let child = RlmTreeCost::new();
        child.inherit(inherited);
        assert!(
            child.has_ceiling(),
            "admissions persist under an inherited cap"
        );
        // Clearing or raising its own ceiling never lifts the inherited one.
        child.set(None, None);
        child.set(Some(100.0), Some(50));
        child.note_spawn_admitted();
        let refusal = child
            .check_spawn(None)
            .expect_err("the inherited limit binds");
        assert!(
            refusal.contains("parent tree's remaining spawn limit (1)"),
            "{refusal}"
        );
        let spender = RlmTreeCost::new();
        spender.inherit(inherited);
        spender.note_spend(3.5);
        let refusal = spender
            .check_spawn(None)
            .expect_err("the inherited USD binds");
        assert!(
            refusal.contains("parent tree's remaining cost ceiling ($3.50)"),
            "{refusal}"
        );
        // A tighter own ceiling still binds first.
        let tight = RlmTreeCost::new();
        tight.inherit(inherited);
        tight.set(Some(1.0), None);
        tight.note_spend(1.0);
        assert!(tight.check_spawn(None).is_err());
        // Unpriced models are refused under an inherited USD cap too.
        let unpriced = RlmTreeCost::new();
        unpriced.inherit(inherited);
        assert!(unpriced.check_spawn(Some(false)).is_err());
    }

    #[test]
    fn grandchildren_inherit_what_is_left_of_the_tighter_budget() {
        let child = RlmTreeCost::new();
        child.inherit(InheritedCostCeiling {
            cost_usd: Some(2.0),
            spawn_limit: None,
        });
        child.set(None, Some(4));
        child.note_spend(0.5);
        assert_eq!(
            child.for_child(),
            Some(InheritedCostCeiling {
                cost_usd: Some(1.5),
                spawn_limit: Some(3),
            }),
            "USD from the inherited cap, spawns from the child's own"
        );
        child.note_spend(5.0);
        assert_eq!(
            child.for_child().and_then(|ceiling| ceiling.cost_usd),
            Some(0.0),
            "an exhausted budget hands down zero, never a negative"
        );
    }

    #[test]
    fn the_inherited_budget_rides_the_wire_in_camel_case() {
        let ceiling = InheritedCostCeiling {
            cost_usd: Some(1.25),
            spawn_limit: None,
        };
        let wire = serde_json::to_value(ceiling).unwrap();
        assert_eq!(wire, json!({"costUsd": 1.25, "spawnLimit": null}));
        assert_eq!(
            serde_json::from_value::<InheritedCostCeiling>(wire).unwrap(),
            ceiling
        );
    }

    #[test]
    fn the_parent_model_priced_default_allows() {
        let tree = RlmTreeCost::new();
        tree.set(Some(1.0), None);
        assert_eq!(tree.check_spawn(Some(tree.parent_model_priced())), Ok(()));
        tree.set_parent_model_priced(false);
        assert!(tree.check_spawn(Some(tree.parent_model_priced())).is_err());
        // A live switch back to a priced model re-admits.
        tree.set_parent_model_priced(true);
        assert_eq!(tree.check_spawn(Some(tree.parent_model_priced())), Ok(()));
    }
}
