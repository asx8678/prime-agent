//! The compaction carry-forward list: a small, bounded set of notes the
//! deterministic engine renders into every summary until cleared and the
//! LLM engine receives with its custom instructions.
//!
//! The list lives in the session log as a custom row ([`COMPACT_CARRY_CUSTOM_TYPE`]),
//! so the latest row on the active branch is the current value: reload,
//! restart, and branch navigation replay it by reading the branch, and no
//! in-memory copy can go stale. Unlike the one-shot `preserve` of a
//! compaction request, carried notes persist across compactions.

use pa_types::session::FileEntry;
use serde_json::Value;

use super::structural_compaction::bounds::one_line;

/// The custom-row type the current list is persisted in.
pub(crate) const COMPACT_CARRY_CUSTOM_TYPE: &str = "compaction_carry";

/// The most carried notes (fabric `MAX_CARRY_ITEMS`).
pub(crate) const MAX_CARRY_ITEMS: usize = 16;
/// The character budget per carried note (fabric `MAX_CARRY_ITEM_CHARS`).
pub(crate) const MAX_CARRY_ITEM_CHARS: usize = 2_048;

/// The bounds every persisted list satisfies: at most [`MAX_CARRY_ITEMS`]
/// notes, each one line of at most [`MAX_CARRY_ITEM_CHARS`] characters.
pub(crate) fn carry_bounds_error(items: &[String]) -> Option<&'static str> {
    if items.len() > MAX_CARRY_ITEMS {
        return Some("compact carry exceeds 16 items");
    }
    for item in items {
        if one_line(item, MAX_CARRY_ITEM_CHARS).is_empty() {
            return Some("compact carry items must be non-empty");
        }
        if item.chars().count() > MAX_CARRY_ITEM_CHARS || item.len() > MAX_CARRY_ITEM_CHARS {
            return Some("compact carry item exceeds 2048 characters or 2048 UTF-8 bytes");
        }
    }
    None
}

/// The details of a carry row: `{version: 1, items}`.
#[must_use]
pub(crate) fn carry_row_details(items: &[String]) -> Value {
    serde_json::json!({ "version": 1, "items": items })
}

/// The durable-row wire shape a carry update persists as: the
/// `custom_message` row the persistence owner's mirror writes when the
/// engine's own manager does not own the file.
#[must_use]
pub(crate) fn carry_row_wire(items: &[String]) -> Value {
    serde_json::json!({
        "customType": COMPACT_CARRY_CUSTOM_TYPE,
        "content": carry_row_content(items),
        "display": false,
        "details": carry_row_details(items),
    })
}

/// The row body, self-describing in the JSONL.
#[must_use]
pub(crate) fn carry_row_content(items: &[String]) -> String {
    let lines = items
        .iter()
        .map(|item| format!("- {}", one_line(item, MAX_CARRY_ITEM_CHARS)))
        .collect::<Vec<_>>();
    format!("[compact carry]\n{}", lines.join("\n"))
}

/// Decode a carry row's details. Fail closed: a malformed row, or a list
/// that violates the bounds, decodes as an empty list — never as an older
/// value the user may have replaced.
#[must_use]
pub(crate) fn decode_carry_details(details: Option<&Value>) -> Vec<String> {
    let details = details.unwrap_or(&Value::Null);
    let items = details
        .get("version")
        .and_then(Value::as_u64)
        .filter(|version| *version == 1)
        .and_then(|_| details.get("items"))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect::<Vec<_>>()
        });
    match items {
        Some(items) if carry_bounds_error(&items).is_none() => items,
        _ => Vec::new(),
    }
}

/// The current list: the latest carry row on the branch wins, including an
/// empty (cleared) list. Rows off the active branch never answer.
#[must_use]
pub(crate) fn latest_carry_items(entries: &[FileEntry]) -> Vec<String> {
    for entry in entries.iter().rev() {
        let FileEntry::CustomMessage { payload, .. } = entry else {
            continue;
        };
        if payload.custom_type != COMPACT_CARRY_CUSTOM_TYPE {
            continue;
        }
        return decode_carry_details(payload.details.as_ref());
    }
    Vec::new()
}

/// One `compact.carry` update: `clear`, `items` (replace), `remove`
/// (exact string), `add` (deduplicated), applied in that order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CarryUpdate {
    pub(crate) clear: bool,
    pub(crate) items: Option<Vec<String>>,
    pub(crate) remove: Vec<String>,
    pub(crate) add: Vec<String>,
}

impl CarryUpdate {
    /// Parse and validate a `compact.carry` payload: absent fields mean no
    /// operation for that field; a payload with no operation reads the
    /// list. Each item must be non-empty within the per-item budget, and
    /// each list within the count bound.
    ///
    /// # Errors
    ///
    /// Returns the first violation, named like the fabric schema errors.
    pub(crate) fn from_payload(data: &Value) -> Result<CarryUpdate, String> {
        let object = data
            .as_object()
            .ok_or_else(|| "compact.carry arguments must be an object".to_string())?;
        let list = |key: &str| -> Result<Option<Vec<String>>, String> {
            let Some(value) = object.get(key) else {
                return Ok(None);
            };
            let items = value
                .as_array()
                .ok_or_else(|| format!("compact.carry {key} must be an array of strings"))?;
            if items.len() > MAX_CARRY_ITEMS {
                return Err(format!(
                    "compact.carry {key} exceeds {MAX_CARRY_ITEMS} items"
                ));
            }
            let parsed = items
                .iter()
                .map(|item| {
                    item.as_str()
                        .map(str::to_string)
                        .ok_or_else(|| format!("compact.carry {key} must be an array of strings"))
                })
                .collect::<Result<Vec<_>, _>>()?;
            if let Some(error) = carry_bounds_error(&parsed) {
                return Err(error.to_string());
            }
            Ok(Some(parsed))
        };
        let clear = match object.get("clear") {
            None => false,
            Some(value) => value
                .as_bool()
                .ok_or_else(|| "compact.carry clear must be a boolean".to_string())?,
        };
        Ok(CarryUpdate {
            clear,
            items: list("items")?,
            remove: list("remove")?.unwrap_or_default(),
            add: list("add")?.unwrap_or_default(),
        })
    }

    /// Whether any operation is present (an empty payload reads the list).
    #[must_use]
    pub(crate) fn is_update(&self) -> bool {
        self.clear || self.items.is_some() || !self.remove.is_empty() || !self.add.is_empty()
    }
}

/// Apply an update to the current list: clear, replace, remove, then add;
/// removal is by exact string, additions skip items already present, and
/// the result is deduplicated. The result must satisfy the bounds.
///
/// # Errors
///
/// Returns the bounds error when the resulting list exceeds the count or
/// per-item budgets.
pub(crate) fn apply_carry_update(
    current: &[String],
    update: &CarryUpdate,
) -> Result<Vec<String>, String> {
    let mut next = if update.clear {
        Vec::new()
    } else {
        current.to_vec()
    };
    if let Some(items) = &update.items {
        next.clone_from(items);
    }
    if !update.remove.is_empty() {
        next.retain(|item| !update.remove.contains(item));
    }
    for item in &update.add {
        if !next.contains(item) {
            next.push(item.clone());
        }
    }
    next.dedup();
    if let Some(error) = carry_bounds_error(&next) {
        return Err(error.to_string());
    }
    Ok(next)
}

/// The sink a host runtime that owns persistence outside the session
/// manager (the daemon worker) subscribes to: the `custom_message` wire row
/// of a carry update, so it lands in the durable file even where the
/// engine's manager is non-persisted.
pub type CarryRowSink = std::sync::Arc<dyn Fn(Value) + Send + Sync>;

/// The summary lines a carried list renders as, one per note with its
/// drop index (fabric `carryRequestLines`).
#[must_use]
pub(crate) fn carry_lines(items: &[String]) -> Vec<String> {
    items
        .iter()
        .enumerate()
        .map(|(index, note)| {
            let note = one_line(note, MAX_CARRY_ITEM_CHARS);
            format!("- {note} [carry:{index}]")
        })
        .collect()
}

/// The summary lines a one-shot preserve list renders as (fabric
/// `requestLines`).
#[must_use]
pub(crate) fn preserve_lines(items: &[String]) -> Vec<String> {
    items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let item = one_line(item, MAX_CARRY_ITEM_CHARS);
            format!("- {item} [preserve:{index}]")
        })
        .collect()
}

/// The instructions the LLM summarizer receives: the user's guidance plus
/// the carry-forward notes (kept in every future summary) and the one-shot
/// preserve items, named like the blocks the deterministic engine renders.
#[must_use]
pub(crate) fn augment_instructions(
    base: Option<&str>,
    carried: &[String],
    preserve: &[String],
) -> Option<String> {
    if base.is_none() && carried.is_empty() && preserve.is_empty() {
        return None;
    }
    let mut parts = Vec::new();
    if let Some(base) = base.map(str::trim).filter(|base| !base.is_empty()) {
        parts.push(base.to_string());
    }
    if !carried.is_empty() {
        parts.push(format!(
            "Carry these notes forward into every future summary, verbatim, under a \"[Carried notes]\" block:\n{}",
            carry_lines(carried).join("\n")
        ));
    }
    if !preserve.is_empty() {
        parts.push(format!(
            "Preserve these items verbatim in this summary only:\n{}",
            preserve_lines(preserve).join("\n")
        ));
    }
    Some(parts.join("\n\n"))
}

/// Validate a `preserve` (or any named string-list) argument: an array of
/// non-empty strings within the item and count budgets (fabric validates
/// `preserve` with the same bounds as carry).
///
/// # Errors
///
/// Returns the first violation.
pub(crate) fn validate_items(
    value: &Value,
    key: &str,
    action: &str,
) -> Result<Vec<String>, String> {
    let items = value
        .as_array()
        .ok_or_else(|| format!("{action} {key} must be an array of strings"))?;
    let parsed = items
        .iter()
        .map(|item| {
            item.as_str()
                .map(str::to_string)
                .ok_or_else(|| format!("{action} {key} must be an array of strings"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if let Some(error) = carry_bounds_error(&parsed) {
        return Err(format!("{action} {key}: {error}"));
    }
    Ok(parsed)
}

/// The list with the note at `index` removed (`compact.drop`).
///
/// # Errors
///
/// Returns an out-of-range error naming the current length.
pub(crate) fn drop_carry_index(current: &[String], index: usize) -> Result<Vec<String>, String> {
    if index >= current.len() {
        return Err(format!(
            "compact.drop index {index} is out of range for {} carried note(s)",
            current.len()
        ));
    }
    let mut next = current.to_vec();
    next.remove(index);
    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    fn carry_row(id: &str, details: &Value) -> String {
        serde_json::json!({
            "type": "custom_message",
            "id": id,
            "customType": COMPACT_CARRY_CUSTOM_TYPE,
            "content": "[compact carry]",
            "display": false,
            "details": details,
        })
        .to_string()
    }

    fn entries(jsonl: &str) -> Vec<FileEntry> {
        jsonl
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).expect("a fixture line parses"))
            .collect()
    }

    #[test]
    fn carry_bounds_reject_overage_and_empty_items() {
        let many = (0..17)
            .map(|index| format!("note {index}"))
            .collect::<Vec<_>>();
        assert_eq!(
            carry_bounds_error(&many),
            Some("compact carry exceeds 16 items")
        );
        assert_eq!(carry_bounds_error(&items(&["a"])), None);
        assert_eq!(
            carry_bounds_error(&items(&["a", "  "])),
            Some("compact carry items must be non-empty")
        );
    }

    #[test]
    fn decode_fails_closed_on_malformed_details() {
        assert_eq!(decode_carry_details(None), Vec::<String>::new());
        assert_eq!(
            decode_carry_details(Some(&serde_json::json!({"items": ["a"]}))),
            Vec::<String>::new(),
            "a row without version 1 decodes as empty"
        );
        assert_eq!(
            decode_carry_details(Some(&serde_json::json!({"version": 2, "items": ["a"]}))),
            Vec::<String>::new()
        );
        assert_eq!(
            decode_carry_details(Some(&serde_json::json!({"version": 1, "items": [1, "a"]}))),
            items(&["a"]),
            "non-string items drop"
        );
        let many = (0..17).map(|index| format!("n{index}")).collect::<Vec<_>>();
        assert_eq!(
            decode_carry_details(Some(&serde_json::json!({"version": 1, "items": many}))),
            Vec::<String>::new(),
            "an out-of-bounds persisted list fails closed"
        );
        assert_eq!(
            decode_carry_details(Some(&serde_json::json!({"version": 1, "items": ["a"]}))),
            items(&["a"])
        );
    }

    #[test]
    fn update_order_is_clear_replace_remove_add() {
        let current = items(&["a", "b"]);
        let update = CarryUpdate {
            clear: false,
            items: None,
            remove: items(&["b"]),
            add: items(&["c", "a", "c"]),
        };
        assert_eq!(
            apply_carry_update(&current, &update).unwrap(),
            items(&["a", "c"])
        );
        let replace = CarryUpdate {
            clear: true,
            items: Some(items(&["x"])),
            remove: items(&["x"]),
            add: items(&["x"]),
        };
        assert_eq!(
            apply_carry_update(&current, &replace).unwrap(),
            items(&["x"]),
            "clear empties, replace sets, remove takes, add puts back"
        );
        let clear_only = CarryUpdate {
            clear: true,
            ..CarryUpdate::default()
        };
        assert_eq!(
            apply_carry_update(&current, &clear_only).unwrap(),
            Vec::<String>::new()
        );
    }

    #[test]
    fn update_rejects_a_result_over_the_bounds() {
        let current = (0..16).map(|index| format!("n{index}")).collect::<Vec<_>>();
        let update = CarryUpdate {
            add: items(&["extra"]),
            ..CarryUpdate::default()
        };
        assert_eq!(
            apply_carry_update(&current, &update),
            Err("compact carry exceeds 16 items".to_string())
        );
    }

    #[test]
    fn drop_removes_by_index_and_names_the_range() {
        assert_eq!(
            drop_carry_index(&items(&["a", "b"]), 0).unwrap(),
            items(&["b"])
        );
        assert_eq!(
            drop_carry_index(&items(&["a"]), 0).unwrap(),
            Vec::<String>::new()
        );
        assert_eq!(
            drop_carry_index(&items(&["a"]), 1),
            Err("compact.drop index 1 is out of range for 1 carried note(s)".to_string())
        );
    }

    #[test]
    fn payload_parsing_validates_types_and_bounds() {
        let parsed = CarryUpdate::from_payload(&serde_json::json!({"add": ["note"]})).unwrap();
        assert_eq!(parsed.add, items(&["note"]));
        assert!(parsed.is_update());
        assert!(!CarryUpdate::from_payload(&serde_json::json!({}))
            .unwrap()
            .is_update());
        assert!(CarryUpdate::from_payload(&serde_json::json!("x")).is_err());
        assert!(CarryUpdate::from_payload(&serde_json::json!({"add": "note"})).is_err());
        assert!(CarryUpdate::from_payload(&serde_json::json!({"clear": "yes"})).is_err());
        let many: Vec<String> = (0..17).map(|index| format!("n{index}")).collect();
        assert!(
            CarryUpdate::from_payload(&serde_json::json!({ "add": many })).is_err(),
            "a 17-item argument is rejected at parse"
        );
        let long = "x".repeat(MAX_CARRY_ITEM_CHARS + 1);
        assert!(CarryUpdate::from_payload(&serde_json::json!({ "add": [long] })).is_err());
    }

    #[test]
    fn latest_row_wins_even_when_older_rows_are_valid() {
        let log = format!(
            "{}\n{}\n{}",
            carry_row("e1", &serde_json::json!({"version": 1, "items": ["old"]})),
            serde_json::json!({"type": "message", "id": "e2", "role": "user", "content": "hi"}),
            carry_row(
                "e3",
                &serde_json::json!({"version": 1, "items": ["new a", "new b"]})
            ),
        );
        assert_eq!(
            latest_carry_items(&entries(&log)),
            items(&["new a", "new b"])
        );

        let cleared = format!(
            "{}\n{}",
            carry_row("e1", &serde_json::json!({"version": 1, "items": ["old"]})),
            carry_row("e2", &serde_json::json!({"version": 1, "items": []})),
        );
        assert_eq!(latest_carry_items(&entries(&cleared)), Vec::<String>::new());

        let malformed = format!(
            "{}\n{}",
            carry_row("e1", &serde_json::json!({"version": 1, "items": ["old"]})),
            carry_row("e2", &serde_json::json!({"version": 9})),
        );
        assert_eq!(
            latest_carry_items(&entries(&malformed)),
            Vec::<String>::new(),
            "a malformed latest row fails closed rather than resurrecting the old value"
        );
        assert_eq!(latest_carry_items(&[]), Vec::<String>::new());
    }

    #[test]
    fn carry_and_preserve_lines_and_instruction_augmentation() {
        let carried = items(&["keep the API stable", "repro in tests/e2e"]);
        assert_eq!(
            carry_lines(&carried),
            items(&[
                "- keep the API stable [carry:0]",
                "- repro in tests/e2e [carry:1]"
            ])
        );
        assert_eq!(
            preserve_lines(&items(&["the error text"])),
            items(&["- the error text [preserve:0]"])
        );
        assert_eq!(augment_instructions(None, &[], &[]), None);
        assert_eq!(
            augment_instructions(Some("focus on tests"), &carried, &items(&["cmd"])),
            Some(
                "focus on tests\n\nCarry these notes forward into every future summary, verbatim, under a \"[Carried notes]\" block:\n- keep the API stable [carry:0]\n- repro in tests/e2e [carry:1]\n\nPreserve these items verbatim in this summary only:\n- cmd [preserve:0]"
                    .to_string()
            )
        );
        assert_eq!(
            augment_instructions(Some("  "), &carried, &[]),
            Some(
                "Carry these notes forward into every future summary, verbatim, under a \"[Carried notes]\" block:\n- keep the API stable [carry:0]\n- repro in tests/e2e [carry:1]"
                    .to_string()
            ),
            "blank instructions drop out of the composition"
        );
    }

    #[test]
    fn validate_items_applies_the_carry_bounds() {
        assert_eq!(
            validate_items(&serde_json::json!(["a", "b"]), "preserve", "compact.run").unwrap(),
            items(&["a", "b"])
        );
        assert!(validate_items(&serde_json::json!("x"), "preserve", "compact.run").is_err());
        assert!(validate_items(&serde_json::json!([1]), "preserve", "compact.run").is_err());
        let many: Vec<String> = (0..17).map(|index| format!("n{index}")).collect();
        assert_eq!(
            validate_items(&serde_json::json!(many), "preserve", "compact.run"),
            Err("compact.run preserve: compact carry exceeds 16 items".to_string())
        );
    }

    #[test]
    fn row_details_and_content_round_trip_through_decode() {
        let list = items(&["keep the API stable", "repro in tests/e2e"]);
        let details = carry_row_details(&list);
        assert_eq!(decode_carry_details(Some(&details)), list);
        assert_eq!(
            carry_row_content(&list),
            "[compact carry]\n- keep the API stable\n- repro in tests/e2e"
        );
    }
}
