//! Structural compaction: a compaction summary built from typed facts of the
//! session log, without a model call (the `deterministic` compaction engine).
//!
//! Entries become typed events ([`events::normalize`]); events fold into a
//! bounded [`state::State`] that is saved on each compaction row; the state
//! renders to a byte-bounded summary ([`render::render`]). The summary text is
//! never read back: the next compaction folds the saved state further, so a
//! fact is never re-summarized and nothing drifts between compactions.
//! Model-written text (branch summaries, an earlier engine's summary) is
//! carried verbatim within bounds, and recent user directions are quoted.

use pa_types::session::FileEntry;
use serde_json::Value;

pub(crate) mod bounds;
mod events;
mod render;
mod state;
#[cfg(test)]
mod tests;

/// One deterministic compaction's input: the state the previous compaction
/// saved and the events since that compaction's kept boundary.
pub(crate) struct Fold {
    state: state::State,
    events: Vec<events::Event>,
    /// The live carry-forward list (the latest carry row on the whole branch).
    carried: Vec<String>,
}

impl Fold {
    /// `previous_state` and `previous_summary` come from the previous
    /// compaction; `entries` are the active-branch rows from its kept boundary
    /// up to the new cut. A state this version saved is folded further;
    /// otherwise a summary another engine wrote is carried once as a labeled
    /// excerpt. `carried` is the live carry list read from the whole branch: it
    /// replaces whatever the saved state and the fold window say, because a pin
    /// or drop outside the window (or under another engine's compaction, which
    /// saves no state) is otherwise lost.
    pub(crate) fn new<'a>(
        previous_state: Option<&Value>,
        previous_summary: Option<&str>,
        entries: impl IntoIterator<Item = &'a FileEntry>,
        carried: Vec<String>,
    ) -> Fold {
        // No earlier summary starts from an empty one: a fresh state.
        let state = previous_state
            .and_then(state::State::from_value)
            .unwrap_or_else(|| state::State::from_summary(previous_summary.unwrap_or_default()));
        Fold {
            state,
            events: events::normalize(entries),
            carried,
        }
    }

    /// The summary, quoting `request` (the `/compact` instructions), and the
    /// state to save on the new compaction row. The rendered state takes at
    /// most 32 KiB and at most half the model's context window in bytes,
    /// about an eighth of it in tokens, so a small window keeps room for the
    /// kept messages. The file-list block appended after it (itself capped
    /// by `FILE_LIST_MAX_ENTRIES` / `FILE_LIST_MAX_COMBINED_CHARS` in
    /// `compaction_utils.rs`) is outside this bound. Fails only when the state cannot
    /// serialize, so no compaction row ever saves a broken state.
    pub(crate) fn summarize(
        &self,
        request: Option<&str>,
        preserve: &[String],
        context_window: u64,
    ) -> serde_json::Result<(String, Value)> {
        let mut state = self.state.clone();
        state.fold(&self.events);
        state.carried.clone_from(&self.carried);
        // An unknown window (zero) leaves the default.
        let max_bytes = usize::try_from(context_window / 2)
            .ok()
            .filter(|half_window| *half_window > 0)
            .map_or(render::DEFAULT_MAX_BYTES, |half_window| {
                half_window.min(render::DEFAULT_MAX_BYTES)
            });
        let saved = state.to_value()?;
        Ok((render::render(&state, request, preserve, max_bytes), saved))
    }
}
