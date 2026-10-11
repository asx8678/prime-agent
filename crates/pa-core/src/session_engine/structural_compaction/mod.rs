//! Structural compaction: a compaction summary built from typed facts of the
//! session log, without a model call.
//!
//! Entries become typed events ([`events::normalize`]); events fold into a
//! bounded [`state::State`] that is saved on each compaction row; the state
//! renders to a byte-bounded summary ([`render::render`]). The summary text is
//! never read back: the next compaction folds the saved state further, so a
//! fact is never re-summarized and nothing drifts between compactions.
//! Model-written text (branch summaries, an earlier engine's summary) is
//! carried verbatim within bounds, and recent user directions are quoted.
#![allow(
    dead_code,
    reason = "wired into compaction by the compaction engine setting in the follow-up change"
)]

mod bounds;
mod events;
mod render;
mod state;
#[cfg(test)]
mod tests;
