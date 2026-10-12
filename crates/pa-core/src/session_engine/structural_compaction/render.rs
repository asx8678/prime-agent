//! The structural summary text. Protected blocks (the header, the recent
//! dialogue, an explicit compaction request) render first and keep every
//! item; history blocks share what is left of the byte budget in proportion
//! to their demand, each under its own ceiling; the footer closes the text.

use super::bounds::{clip_bytes, Excerpt};
use super::state::{BlockOrigin, Exchange, State, EXCHANGE_TEXT_BYTES};

/// The default summary budget in UTF-8 bytes.
pub(crate) const DEFAULT_MAX_BYTES: usize = 32 * 1024;

pub(super) const HEADER: &str = "[Deterministic summary] Built from the session log without a model: older facts are sampled, not interpreted. uN is the Nth user message and uN.cM the Mth Python cell after it.";
const DIALOGUE_HEADER: &str = "[Recent user directions and discussion]";
const DIALOGUE_MAX_BYTES: usize = 12_288;
const REQUEST_HEADER: &str = "[Compaction Request]";
const REQUEST_MAX_BYTES: usize = 3_072;
const CARRIED_HEADER: &str = "[Carried notes]";
const CARRIED_MAX_BYTES: usize = 3_072;
const FOOTER_MAX_BYTES: usize = 1_536;
const MAX_LINE_BYTES: usize = 2_048;
const MAX_LINES_PER_BLOCK: usize = 128;
/// The sequence that would end the `<summary>` wrapper the model reads.
const SUMMARY_END: &str = "</summary>";
const SUMMARY_END_ESCAPED: &str = "<\\/summary>";

struct HistoryBlock {
    header: &'static str,
    lines: Vec<String>,
    max_bytes: usize,
}

/// The summary of `state`, with `request` (the `/compact` instructions)
/// quoted verbatim, in at most `max_bytes` UTF-8 bytes.
#[must_use]
pub(crate) fn render(
    state: &State,
    request: Option<&str>,
    preserve: &[String],
    max_bytes: usize,
) -> String {
    if max_bytes == 0 {
        return String::new();
    }
    let mut blocks = vec![HEADER.to_string()];
    blocks.extend(dialogue(state));
    let mut request_items = Vec::new();
    if let Some(request) = request.map(str::trim).filter(|request| !request.is_empty()) {
        request_items.push(request.to_string());
    }
    request_items.extend(crate::session_engine::compact_carry::preserve_lines(
        preserve,
    ));
    if !request_items.is_empty() {
        blocks.push(protected_items(
            REQUEST_HEADER,
            &request_items,
            REQUEST_MAX_BYTES,
        ));
    }
    // The carry-forward list renders into every summary until cleared.
    if !state.carried.is_empty() {
        let lines = crate::session_engine::compact_carry::carry_lines(&state.carried);
        blocks.push(protected_items(CARRIED_HEADER, &lines, CARRIED_MAX_BYTES));
    }
    let history = history_blocks(state);
    let footer = bounded_block("---", &[footer_line(state)], FOOTER_MAX_BYTES);
    let framing = footer.len() + 2 * (blocks.len() + history.len()) + 1;
    let protected: usize = blocks.iter().map(String::len).sum();
    blocks.extend(bounded_history(
        &history,
        max_bytes.saturating_sub(framing + protected),
    ));
    blocks.push(footer);
    let summary = format!("{}\n", blocks.join("\n\n")).replace(SUMMARY_END, SUMMARY_END_ESCAPED);
    if summary.len() <= max_bytes {
        summary
    } else {
        format!("{}\n", clip_bytes(&summary, max_bytes.saturating_sub(1)))
    }
}

/// The recent exchanges and the latest reply, each rendered within a budget
/// chosen before rendering: everything at full size when it fits, otherwise
/// equal shares with the spare going to the newest items first. Budgeting up
/// front keeps the user's words; cutting rendered items would drop them first.
fn dialogue(state: &State) -> Option<String> {
    let items = state.exchanges.len() + usize::from(state.reply.is_some());
    if items == 0 {
        return None;
    }
    let render_item = |index: usize, budget: usize| {
        match state.exchanges.get(index) {
        Some(exchange) => exchange_text(exchange, budget),
        None => state
            .reply
            .as_ref()
            .map(|reply| {
                let heading = format!(
                    "Latest assistant response before the kept messages, not a verified outcome [{}]:",
                    reply.at
                );
                let text = reply.text.render(budget.saturating_sub(heading.len() + 1));
                format!("{heading}\n{text}")
            })
            .unwrap_or_default(),
    }
    };
    let available = DIALOGUE_MAX_BYTES.saturating_sub(DIALOGUE_HEADER.len() + 2 * items);
    let wanted: Vec<usize> = (0..items)
        .map(|index| render_item(index, EXCHANGE_TEXT_BYTES).len())
        .collect();
    let budgets = if wanted.iter().sum::<usize>() <= available {
        wanted
    } else {
        fair_shares(&wanted, available)
    };
    let rendered: Vec<String> = budgets
        .into_iter()
        .enumerate()
        .map(|(index, budget)| render_item(index, budget))
        .collect();
    Some(format!("{DIALOGUE_HEADER}\n\n{}", rendered.join("\n\n")))
}

/// Equal shares of `available`, never more than an item wants; what is left
/// over goes to the last items first.
fn fair_shares(wanted: &[usize], available: usize) -> Vec<usize> {
    let share = available / wanted.len().max(1);
    let mut budgets: Vec<usize> = wanted.iter().map(|want| (*want).min(share)).collect();
    let mut leftover = available.saturating_sub(budgets.iter().sum());
    for (budget, want) in budgets.iter_mut().zip(wanted).rev() {
        let extra = leftover.min(want - *budget);
        *budget += extra;
        leftover -= extra;
    }
    budgets
}

/// One exchange in at most `budget` bytes: the user's words get at least
/// half, a short reply leaves the rest to the assistant text before it.
fn exchange_text(exchange: &Exchange, budget: usize) -> String {
    let user_heading = format!("User instruction [{}]:", exchange.user.at);
    let Some(reply) = &exchange.reply else {
        let text = exchange
            .user
            .text
            .render(budget.saturating_sub(user_heading.len() + 1));
        return format!("{user_heading}\n{text}");
    };
    let reply_heading = format!(
        "Historical assistant response, not a verified outcome [{}]:",
        reply.at
    );
    let available = budget.saturating_sub(user_heading.len() + reply_heading.len() + 4);
    let user_budget = (available / 2).max(available.saturating_sub(reply.text.text.len()));
    let user = exchange.user.text.render(user_budget);
    let reply_text = reply.text.render(available.saturating_sub(user.len()));
    format!("{reply_heading}\n{reply_text}\n\n{user_heading}\n{user}")
}

fn history_blocks(state: &State) -> Vec<HistoryBlock> {
    let protected: Vec<&str> = state
        .exchanges
        .iter()
        .map(|exchange| exchange.user.at.as_str())
        .collect();
    let requests = state.requests.lines("earlier requests", |line| {
        (!protected.contains(&line.at.as_str())).then(|| format!("- {} [{}]", line.text, line.at))
    });
    let summaries = state.blocks.lines("summaries", |block| {
        let origin = match block.origin {
            BlockOrigin::BranchSummary => "Branch summary, model-written",
            BlockOrigin::EarlierSummary => "Earlier compaction summary, model-written",
            BlockOrigin::EarlierDeterministic => "Earlier compaction summary, deterministic",
            BlockOrigin::Fold => "Context fold result, model-written",
        };
        Some(format!(
            "{origin} [{}]:\n{}",
            block.at,
            block.text.render(MAX_LINE_BYTES)
        ))
    });
    let activity = state.activity.lines("activity records", |line| {
        Some(format!("- {} [{}]", line.text, line.at))
    });
    let turns = state.turns.lines("earlier turns", |line| {
        Some(format!("- {} [{}]", line.text, line.at))
    });
    let mut transcript: Vec<String> = Vec::new();
    if state.transcript_omitted > 0 {
        transcript.push(format!(
            "\u{2026} {} earlier events not shown",
            state.transcript_omitted
        ));
    }
    transcript.extend(
        state
            .transcript
            .iter()
            .map(|line| format!("{} {}", line.at, line.text)),
    );
    [
        ("[Historical requests]", requests, 4_096),
        ("[Earlier summaries]", summaries, 4_096),
        ("[Agents And Activity]", activity, 2_048),
        ("[Outstanding Context]", outstanding(state), 4_608),
        ("[Earlier Turns]", turns, 3_072),
        ("[Current Status]", status(state), 2_048),
        ("---", transcript, 5_120),
    ]
    .into_iter()
    .filter(|(_, lines, _)| !lines.is_empty())
    .map(|(header, lines, max_bytes)| HistoryBlock {
        header,
        lines,
        max_bytes,
    })
    .collect()
}

fn outstanding(state: &State) -> Vec<String> {
    let mut lines: Vec<String> = state
        .open_failures
        .iter()
        .map(|failure| format!("- {}: {} [{}]", failure.subject, failure.error, failure.at))
        .collect();
    if state.open_failures_omitted > 0 {
        let middle = lines.len() / 2;
        lines.insert(
            middle,
            format!(
                "\u{2026} {} more open failures not shown",
                state.open_failures_omitted
            ),
        );
    }
    if state.resolved_failures_omitted > 0 {
        lines.push(format!(
            "\u{2026} {} earlier resolved failures not shown",
            state.resolved_failures_omitted
        ));
    }
    lines.extend(state.resolved_failures.iter().map(|failure| {
        format!(
            "- {}: {} [resolved] [{}]",
            failure.subject, failure.error, failure.at
        )
    }));
    lines
}

fn status(state: &State) -> Vec<String> {
    let status = &state.status;
    [
        status
            .request
            .as_ref()
            .map(|line| format!("Last request [{}]: {}", line.at, line.text)),
        state.turn.as_ref().map(|turn| {
            let line = turn.line();
            format!("Open turn [{}]: {}", line.at, line.text)
        }),
        status
            .change
            .as_ref()
            .map(|line| format!("Last edited file [{}]: {}", line.at, line.text)),
        status
            .action
            .as_ref()
            .map(|line| format!("Last action [{}]: {}", line.at, line.text)),
        status
            .note
            .as_ref()
            .map(|line| format!("Last assistant note [{}]: {}", line.at, line.text)),
    ]
    .into_iter()
    .flatten()
    .collect()
}

fn footer_line(state: &State) -> String {
    let range = if state.users == 0 {
        "u0".to_string()
    } else {
        format!("u1\u{2013}u{}", state.users)
    };
    format!(
        "[Deterministic summary of {range}: events folded: {}; thinking blocks not carried: {}; the full history stays in the session log.]",
        state.events, state.thinking
    )
}

/// `header` and as many `lines` as fit in `max_bytes`, keeping the earliest
/// and latest lines and marking the gap.
fn bounded_block(header: &str, lines: &[String], max_bytes: usize) -> String {
    let clipped: Vec<String> = lines
        .iter()
        .map(|line| clip_bytes(line, MAX_LINE_BYTES))
        .collect();
    let capped = sampled_lines(&clipped, clipped.len().min(MAX_LINES_PER_BLOCK));
    for keep in (0..=capped.len()).rev() {
        let block = std::iter::once(header.to_string())
            .chain(sampled_lines(&capped, keep))
            .collect::<Vec<_>>()
            .join("\n");
        if block.len() <= max_bytes {
            return block;
        }
    }
    clip_bytes(header, max_bytes)
}

fn sampled_lines(lines: &[String], keep: usize) -> Vec<String> {
    if lines.len() <= keep {
        return lines.to_vec();
    }
    let latest = keep / 2;
    let mut sampled = lines[..keep.div_ceil(2)].to_vec();
    sampled.push(format!(
        "\u{2026} omitted {} rendered lines",
        lines.len() - keep
    ));
    sampled.extend_from_slice(&lines[lines.len() - latest..]);
    sampled
}

/// A protected multi-item block (fabric `protectedBlock`): every item
/// stays, excerpted proportionally only when the whole block exceeds its
/// allocation; what is left after the equal shares goes to the earliest
/// items that can use it.
fn protected_items(header: &str, items: &[String], max_bytes: usize) -> String {
    let joined = format!("{header}\n\n{}", items.join("\n\n"));
    if joined.len() <= max_bytes {
        return joined;
    }
    let mut remaining = max_bytes.saturating_sub(header.len() + 2 * items.len());
    let share = remaining / items.len().max(1);
    let budgets: Vec<usize> = items.iter().map(|item| item.len().min(share)).collect();
    remaining -= budgets.iter().sum::<usize>();
    let excerpts = items
        .iter()
        .zip(&budgets)
        .map(|(item, budget)| {
            let extra = remaining.min(item.len() - budget);
            remaining -= extra;
            Excerpt::of(item, budget + extra).render(budget + extra)
        })
        .collect::<Vec<_>>();
    format!("{header}\n\n{}", excerpts.join("\n\n"))
}

/// The history blocks within `max_bytes`: each keeps its header and gap
/// marker, and the rest of the space goes to blocks in proportion to the
/// bytes they want.
fn bounded_history(blocks: &[HistoryBlock], max_bytes: usize) -> Vec<String> {
    let candidates: Vec<(String, usize)> = blocks
        .iter()
        .map(|block| {
            let text = bounded_block(block.header, &block.lines, block.max_bytes);
            let minimum = if block.lines.len() == 1 {
                text.len()
            } else {
                let skeleton = std::iter::once(block.header.to_string())
                    .chain(sampled_lines(&block.lines, 0))
                    .collect::<Vec<_>>()
                    .join("\n");
                text.len().min(skeleton.len())
            };
            (text, minimum)
        })
        .collect();
    let total: usize = candidates.iter().map(|(text, _)| text.len()).sum();
    if total <= max_bytes {
        return candidates.into_iter().map(|(text, _)| text).collect();
    }
    let minimum: usize = candidates.iter().map(|(_, minimum)| minimum).sum();
    let mut remaining = max_bytes.saturating_sub(minimum);
    let mut demand = total - minimum;
    blocks
        .iter()
        .zip(candidates)
        .map(|(block, (text, minimum))| {
            let wanted = text.len() - minimum;
            let extra = (remaining * wanted)
                .checked_div(demand)
                .map_or(0, |share| wanted.min(share));
            let text = bounded_block(block.header, &block.lines, minimum + extra);
            remaining = remaining.saturating_sub(text.len().saturating_sub(minimum));
            demand -= wanted;
            text
        })
        .collect()
}
