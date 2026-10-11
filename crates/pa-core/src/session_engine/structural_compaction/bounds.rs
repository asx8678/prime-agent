//! Byte- and count-bounded text for the structural summary: UTF-8-safe
//! clipping, one-line squashing, excerpts that record what they cut, and the
//! streaming earliest-plus-latest sample every long collection uses.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

const ELLIPSIS: &str = "\u{2026}";

/// The largest char boundary at or below `index`.
fn floor_boundary(text: &str, index: usize) -> usize {
    let mut end = index.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    end
}

/// `text` cut to at most `max_bytes` UTF-8 bytes on a char boundary, ending
/// with an ellipsis when anything was cut.
#[must_use]
pub(crate) fn clip_bytes(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    if max_bytes < ELLIPSIS.len() {
        return String::new();
    }
    let end = floor_boundary(text, max_bytes - ELLIPSIS.len());
    format!("{}{ELLIPSIS}", &text[..end])
}

/// The first non-blank line of `text` with whitespace runs collapsed, cut to
/// `max_chars` characters.
#[must_use]
pub(crate) fn one_line(text: &str, max_chars: usize) -> String {
    let line = text
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or_default();
    let squashed = line.split_whitespace().collect::<Vec<_>>().join(" ");
    if squashed.chars().count() <= max_chars {
        return squashed;
    }
    let kept: String = squashed.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{kept}{ELLIPSIS}")
}

/// A source text kept whole when it fits, otherwise cut from the end with the
/// number of UTF-8 bytes it lost.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Excerpt {
    pub(crate) text: String,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub(crate) omitted_bytes: u64,
}

impl Excerpt {
    #[must_use]
    pub(crate) fn of(text: &str, max_bytes: usize) -> Excerpt {
        let text = text.trim();
        if text.len() <= max_bytes {
            return Excerpt {
                text: text.to_string(),
                omitted_bytes: 0,
            };
        }
        let end = floor_boundary(text, max_bytes);
        Excerpt {
            text: text[..end].to_string(),
            omitted_bytes: u64::try_from(text.len() - end).unwrap_or(u64::MAX),
        }
    }

    /// The text cut further to `max_bytes`, followed by a marker naming every
    /// byte lost so far; the marker shares the budget and takes no more of it
    /// than its own length.
    #[must_use]
    pub(crate) fn render(&self, max_bytes: usize) -> String {
        if self.omitted_bytes == 0 && self.text.len() <= max_bytes {
            return self.text.clone();
        }
        // The marker's length depends on how much is cut, and the cut on the
        // marker's length; the digit count settles within a few rounds.
        let mut reserve = 0;
        for _ in 0..4 {
            let end = floor_boundary(&self.text, max_bytes.saturating_sub(reserve));
            let lost = u64::try_from(self.text.len() - end)
                .unwrap_or(u64::MAX)
                .saturating_add(self.omitted_bytes);
            let marker = format!("\n[omitted {lost} UTF-8 bytes]");
            if end + marker.len() <= max_bytes {
                return format!("{}{marker}", &self.text[..end]);
            }
            reserve = marker.len();
        }
        clip_bytes(&self.text, max_bytes)
    }
}

#[allow(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde's skip_serializing_if passes the field by reference"
)]
fn is_zero(value: &u64) -> bool {
    *value == 0
}

/// One rendered line with the address of the fact it came from.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Line {
    pub(crate) at: String,
    pub(crate) text: String,
}

impl Line {
    #[must_use]
    pub(crate) fn new(at: &str, text: impl Into<String>) -> Line {
        Line {
            at: at.to_string(),
            text: text.into(),
        }
    }
}

/// Items that carry the address an omission range names.
pub(crate) trait Addressed {
    /// The item's address (`u12`, `u12.c3`).
    fn address(&self) -> &str;
}

impl Addressed for Line {
    fn address(&self) -> &str {
        &self.at
    }
}

/// A streaming earliest-plus-latest sample: the first `ceil(cap / 2)` items
/// stay, the latest `floor(cap / 2)` slide, and every item pushed out of the
/// window is counted with the address range it spanned. Pushing items one at
/// a time gives the same sample however the stream is split.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Sample<T> {
    pub(crate) earliest: Vec<T>,
    pub(crate) latest: VecDeque<T>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub(crate) omitted: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) omitted_first: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) omitted_last: Option<String>,
}

impl<T> Default for Sample<T> {
    fn default() -> Self {
        Sample {
            earliest: Vec::new(),
            latest: VecDeque::new(),
            omitted: 0,
            omitted_first: None,
            omitted_last: None,
        }
    }
}

impl<T: Addressed> Sample<T> {
    pub(crate) fn push(&mut self, item: T, cap: usize) {
        if self.earliest.len() < cap.div_ceil(2) {
            self.earliest.push(item);
            return;
        }
        self.latest.push_back(item);
        if self.latest.len() > cap / 2 {
            if let Some(dropped) = self.latest.pop_front() {
                self.omitted += 1;
                if self.omitted_first.is_none() {
                    self.omitted_first = Some(dropped.address().to_string());
                }
                self.omitted_last = Some(dropped.address().to_string());
            }
        }
    }

    /// The `… omitted N <noun>` marker between the earliest and latest items.
    #[must_use]
    pub(crate) fn omission(&self, noun: &str) -> Option<String> {
        (self.omitted > 0).then(|| {
            format!(
                "\u{2026} omitted {} {noun} ({} \u{2192} {})",
                self.omitted,
                self.omitted_first.as_deref().unwrap_or("?"),
                self.omitted_last.as_deref().unwrap_or("?"),
            )
        })
    }

    /// Rendered lines in order, with the omission marker where the gap is;
    /// items `line` maps to `None` are left out.
    pub(crate) fn lines(&self, noun: &str, line: impl Fn(&T) -> Option<String>) -> Vec<String> {
        let mut lines: Vec<String> = self.earliest.iter().filter_map(&line).collect();
        lines.extend(self.omission(noun));
        lines.extend(self.latest.iter().filter_map(line));
        lines
    }
}
