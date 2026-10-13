//! `/heartbeat` command parsing and the heartbeat schedule/delivery vocabulary, shared by the
//! client that parses the command (pa-tui) and the host that schedules the heartbeat (pa-core's
//! cron). Pure string handling: no I/O, no clock.

use serde::{Deserialize, Serialize};

pub const DEFAULT_HEARTBEAT_SCHEDULE: &str = "every 5m";

pub const DEFAULT_HEARTBEAT_DELIVERY_MODE: DeliveryMode = DeliveryMode::Steer;

pub const ONE_SECOND_MS: u64 = 1000;

pub const ONE_MINUTE_MS: u64 = 60 * ONE_SECOND_MS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryMode {
    Steer,
    FollowUp,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ParsedHeartbeatCommand {
    Status,
    Pause,
    Resume,
    Clear,
    Set {
        schedule: String,
        instruction: String,
        delivery_mode: Option<DeliveryMode>,
    },
}

/// The interval unit's length in ms (`m`, `h`, `d`; `s` only when `allow_seconds`).
#[must_use]
pub fn unit_multiplier(unit: &str, allow_seconds: bool) -> Option<u64> {
    let unit = unit.trim();
    let seconds = allow_seconds && matches!(unit, "s" | "sec" | "secs" | "second" | "seconds");
    if seconds {
        return Some(ONE_SECOND_MS);
    }
    if matches!(unit, "m" | "min" | "mins" | "minute" | "minutes") {
        return Some(ONE_MINUTE_MS);
    }
    if matches!(unit, "h" | "hr" | "hrs" | "hour" | "hours") {
        return Some(60 * ONE_MINUTE_MS);
    }
    if matches!(unit, "d" | "day" | "days") {
        return Some(24 * 60 * ONE_MINUTE_MS);
    }
    None
}

/// `<digits> <unit?>` with the unit optionally attached (`10m`, `10 m`).
#[must_use]
pub fn split_amount_unit(rest: &str) -> Option<(u64, &str)> {
    let rest = rest.trim();
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return None;
    }
    let digits_len = digits.len();
    let unit = rest[digits_len..].trim();
    let amount: u64 = digits.parse().ok()?;
    if !unit.is_empty()
        && !rest[digits_len..].starts_with(char::is_whitespace)
        && !unit.chars().all(|c| c.is_ascii_alphabetic())
    {
        return None;
    }
    Some((amount, unit))
}

pub fn normalize_heartbeat_schedule(input: Option<&str>) -> String {
    let text = input.map(str::trim).filter(|text| !text.is_empty());
    let Some(text) = text else {
        return DEFAULT_HEARTBEAT_SCHEDULE.to_string();
    };
    if let Some((_, unit)) = split_amount_unit(text) {
        if !unit.is_empty() && unit_multiplier(unit, true).is_some() {
            return format!("every {text}");
        }
    }
    text.to_string()
}

/// Normalize an optional heartbeat delivery mode string.
///
/// # Errors
///
/// Returns an error when `value` is `Some` but neither `"steer"` nor `"follow_up"`.
pub fn normalize_heartbeat_delivery_mode(
    value: Option<&str>,
) -> anyhow::Result<Option<DeliveryMode>> {
    match value {
        None => Ok(None),
        Some("steer") => Ok(Some(DeliveryMode::Steer)),
        Some("follow_up") => Ok(Some(DeliveryMode::FollowUp)),
        Some(_) => Err(anyhow::anyhow!(
            "Heartbeat delivery mode must be \"steer\" or \"follow_up\""
        )),
    }
}

/// Parse a `/heartbeat ...` command body.
///
/// # Errors
///
/// Returns an error when the command body is malformed.
pub fn parse_heartbeat_command(input: &str) -> anyhow::Result<ParsedHeartbeatCommand> {
    let text = input
        .strip_prefix("/heartbeat")
        .filter(|rest| rest.chars().next().is_none_or(char::is_whitespace))
        .map_or_else(|| input.trim(), str::trim_start);
    if text.is_empty() || text == "status" {
        return Ok(ParsedHeartbeatCommand::Status);
    }
    if text == "pause" {
        return Ok(ParsedHeartbeatCommand::Pause);
    }
    if text == "resume" {
        return Ok(ParsedHeartbeatCommand::Resume);
    }
    if text == "clear" || text == "stop" {
        return Ok(ParsedHeartbeatCommand::Clear);
    }
    let leading = consume_delivery_option(text)?;
    let mut delivery_mode = leading.0;
    let rest = leading.1;

    if let Some(option) = consume_every_option(&rest)? {
        let trailing = consume_delivery_option(&option.1)?;
        delivery_mode = trailing.0.or(delivery_mode);
        if trailing.1.is_empty() {
            anyhow::bail!(
                "Usage: /heartbeat [--every <interval>] [--steer|--follow-up] <instruction>"
            );
        }
        return Ok(ParsedHeartbeatCommand::Set {
            schedule: normalize_heartbeat_schedule(Some(&option.0)),
            instruction: trailing.1,
            delivery_mode,
        });
    }
    if let Some(leading_schedule) = consume_leading_every_schedule(&rest) {
        let trailing = consume_delivery_option(&leading_schedule.1)?;
        delivery_mode = trailing.0.or(delivery_mode);
        if trailing.1.is_empty() {
            anyhow::bail!(
                "Usage: /heartbeat [--every <interval>] [--steer|--follow-up] <instruction>"
            );
        }
        return Ok(ParsedHeartbeatCommand::Set {
            schedule: normalize_heartbeat_schedule(Some(&leading_schedule.0)),
            instruction: trailing.1,
            delivery_mode,
        });
    }
    let rest = rest.trim();
    if rest.is_empty() {
        anyhow::bail!("Usage: /heartbeat [--every <interval>] [--steer|--follow-up] <instruction>");
    }
    Ok(ParsedHeartbeatCommand::Set {
        schedule: DEFAULT_HEARTBEAT_SCHEDULE.to_string(),
        instruction: rest.to_string(),
        delivery_mode,
    })
}

fn consume_delivery_option(text: &str) -> anyhow::Result<(Option<DeliveryMode>, String)> {
    let mut rest = text.trim().to_string();
    let lower = rest.to_lowercase();
    if lower == "--deliver" || lower.ends_with(" --deliver") {
        return Err(anyhow::anyhow!(
            "Heartbeat delivery mode must be \"steer\" or \"follow_up\""
        ));
    }
    let mut delivery_mode: Option<DeliveryMode> = None;
    while let Some((mode, remaining)) = consume_leading_delivery_flag(&rest)? {
        delivery_mode = Some(mode);
        rest = remaining.trim().to_string();
    }
    let mut trailing_delivery_mode: Option<DeliveryMode> = None;
    while let Some((mode, remaining)) = consume_trailing_delivery_flag(&rest)? {
        // Consume all trailing flags but keep the rightmost (textually latest).
        trailing_delivery_mode = trailing_delivery_mode.or(Some(mode));
        rest = remaining.trim().to_string();
    }
    Ok((trailing_delivery_mode.or(delivery_mode), rest))
}

fn consume_leading_delivery_flag(text: &str) -> anyhow::Result<Option<(DeliveryMode, String)>> {
    let trimmed = text.trim_start();
    let Some(after_dash) = trimmed.strip_prefix("--") else {
        return Ok(None);
    };
    if let Some(value) = after_dash.strip_prefix("deliver") {
        let value = value
            .strip_prefix('=')
            .map(String::from)
            .or_else(|| value.strip_prefix(char::is_whitespace).map(String::from))
            .unwrap_or_default();
        if value.is_empty() {
            anyhow::bail!("Heartbeat delivery mode must be \"steer\" or \"follow_up\"");
        }
        let (token, remainder) = split_first_token(&value);
        let mode = parse_delivery_mode_token(token)?;
        return Ok(Some((mode, remainder.to_string())));
    }
    for (token, mode) in [
        ("steer", DeliveryMode::Steer),
        ("follow-up", DeliveryMode::FollowUp),
        ("follow_up", DeliveryMode::FollowUp),
    ] {
        if let Some(rest) = after_dash.strip_prefix(token) {
            check_standalone(after_dash, token)?;
            return Ok(Some((mode, rest.to_string())));
        }
    }
    Ok(None)
}

fn check_standalone(rest: &str, token: &str) -> anyhow::Result<()> {
    let after = &rest[token.len()..];
    if !after.is_empty() && !after.starts_with(char::is_whitespace) {
        anyhow::bail!("Heartbeat delivery mode must be \"steer\" or \"follow_up\"");
    }
    Ok(())
}

fn split_first_token(value: &str) -> (&str, &str) {
    match value.find(char::is_whitespace) {
        Some(index) => (&value[..index], value[index..].trim_start()),
        None => (value, ""),
    }
}

fn consume_trailing_delivery_flag(text: &str) -> anyhow::Result<Option<(DeliveryMode, String)>> {
    // ASCII lowercasing keeps every byte offset, so indices found in `lower` slice `text`.
    let lower = text.to_ascii_lowercase();
    if let Some(index) = lower.rfind(" --deliver ") {
        let token = text[index + " --deliver ".len()..].trim();
        if !token.is_empty() && !token.contains(char::is_whitespace) {
            return Ok(Some((
                parse_delivery_mode_token(token)?,
                text[..index].to_string(),
            )));
        }
    }
    if let Some(index) = lower.rfind(" --deliver=") {
        let token = text[index + " --deliver=".len()..].trim();
        if !token.is_empty() && !token.contains(char::is_whitespace) {
            return Ok(Some((
                parse_delivery_mode_token(token)?,
                text[..index].to_string(),
            )));
        }
    }
    for token in ["--follow-up", "--follow_up", "--steer"] {
        if let Some(index) = lower.rfind(&format!(" {token}")) {
            if index + token.len() + 1 == text.len() {
                return Ok(Some((
                    parse_delivery_mode_token(&token[2..])?,
                    text[..index].to_string(),
                )));
            }
        }
    }
    Ok(None)
}

fn parse_delivery_mode_token(token: &str) -> anyhow::Result<DeliveryMode> {
    let normalized = token.to_lowercase().replace('-', "_");
    match normalized.as_str() {
        "steer" => Ok(DeliveryMode::Steer),
        "follow_up" => Ok(DeliveryMode::FollowUp),
        _ => Err(anyhow::anyhow!(
            "Heartbeat delivery mode must be \"steer\" or \"follow_up\""
        )),
    }
}

fn consume_every_option(text: &str) -> anyhow::Result<Option<(String, String)>> {
    let trimmed = text.trim_start();
    let Some(rest) = trimmed.strip_prefix("--every") else {
        return Ok(None);
    };
    let rest = match rest.strip_prefix('=') {
        Some(value) => value.to_string(),
        None => match rest.strip_prefix(char::is_whitespace) {
            Some(value) => value.to_string(),
            // No separator: "--everyfoo" is not an every option.
            None if !rest.is_empty() => return Ok(None),
            None => String::new(),
        },
    };
    let (interval, remainder): (String, String) = if let Some(stripped) = rest.strip_prefix('"') {
        match stripped.find('"') {
            Some(index) => (
                stripped[..index].to_string(),
                stripped[index + 1..].to_string(),
            ),
            None => (stripped.to_string(), String::new()),
        }
    } else if let Some(stripped) = rest.strip_prefix('\'') {
        match stripped.find('\'') {
            Some(index) => (
                stripped[..index].to_string(),
                stripped[index + 1..].to_string(),
            ),
            None => (stripped.to_string(), String::new()),
        }
    } else {
        let (token, rest) = split_first_token(&rest);
        (token.to_string(), rest.to_string())
    };
    if interval.is_empty() {
        anyhow::bail!("Usage: /heartbeat [--every <interval>] [--steer|--follow-up] <instruction>");
    }
    Ok(Some((interval, remainder.trim_start().to_string())))
}

fn consume_leading_every_schedule(text: &str) -> Option<(String, String)> {
    let lower = text.to_lowercase();
    for prefix in ["every ", "each "] {
        if let Some(rest) = lower.strip_prefix(prefix) {
            let first_token = rest.split_whitespace().next()?;
            let Some((_, unit)) = split_amount_unit(first_token) else {
                continue;
            };
            if unit.is_empty() || unit_multiplier(unit, true).is_none() {
                continue;
            }
            {
                let consumed = prefix.len() + first_token.len();
                let consumed = consumed.min(text.len());
                let remainder = text[consumed..].trim();
                // Strip a standalone "--" separator, never a flag.
                let remainder = remainder
                    .strip_prefix("--")
                    .filter(|after| after.is_empty() || after.starts_with(char::is_whitespace))
                    .map_or(remainder, str::trim)
                    .trim()
                    .to_string();
                let interval = format!("{} {}", prefix.trim(), first_token);
                return Some((interval, remainder));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heartbeat_command_parsing() {
        use ParsedHeartbeatCommand::*;
        assert_eq!(parse_heartbeat_command("status").unwrap(), Status);
        assert_eq!(parse_heartbeat_command("/heartbeat").unwrap(), Status);
        assert_eq!(
            parse_heartbeat_command("/heartbeat status").unwrap(),
            Status
        );
        assert_eq!(parse_heartbeat_command("/heartbeat pause").unwrap(), Pause);
        assert_eq!(
            parse_heartbeat_command("/heartbeat resume").unwrap(),
            Resume
        );
        assert_eq!(parse_heartbeat_command("/heartbeat stop").unwrap(), Clear);
        let Set {
            schedule,
            instruction,
            delivery_mode,
        } = parse_heartbeat_command("/heartbeat check the build").unwrap()
        else {
            panic!("expected set");
        };
        assert_eq!(schedule, "every 5m");
        assert_eq!(instruction, "check the build");
        assert_eq!(delivery_mode, None);
        let Set {
            schedule,
            instruction,
            ..
        } = parse_heartbeat_command("/heartbeat --every 10m keep going").unwrap()
        else {
            panic!("expected set");
        };
        assert_eq!(schedule, "every 10m");
        assert_eq!(instruction, "keep going");
        let Set {
            schedule,
            instruction,
            ..
        } = parse_heartbeat_command("/heartbeat --every=\"15 m\" watch things").unwrap()
        else {
            panic!("expected set");
        };
        assert_eq!(schedule, "every 15 m");
        assert_eq!(instruction, "watch things");
        let Set {
            schedule,
            instruction,
            ..
        } = parse_heartbeat_command("/heartbeat every 2h sweep logs").unwrap()
        else {
            panic!("expected set");
        };
        assert_eq!(schedule, "every 2h");
        assert_eq!(instruction, "sweep logs");
        let Set { delivery_mode, .. } =
            parse_heartbeat_command("/heartbeat --steer --every 5m ping").unwrap()
        else {
            panic!("expected set");
        };
        assert_eq!(delivery_mode, Some(DeliveryMode::Steer));
        let Set { delivery_mode, .. } =
            parse_heartbeat_command("/heartbeat --every 5m ping --follow-up").unwrap()
        else {
            panic!("expected set");
        };
        assert_eq!(delivery_mode, Some(DeliveryMode::FollowUp));
        let Set { delivery_mode, .. } =
            parse_heartbeat_command("/heartbeat --deliver=steer --every 5m ping").unwrap()
        else {
            panic!("expected set");
        };
        assert_eq!(delivery_mode, Some(DeliveryMode::Steer));
        let Set { delivery_mode, .. } =
            parse_heartbeat_command("/heartbeat --deliver follow_up --every 5m ping").unwrap()
        else {
            panic!("expected set");
        };
        assert_eq!(delivery_mode, Some(DeliveryMode::FollowUp));
        assert!(parse_heartbeat_command("/heartbeat --every 10m").is_err());
    }

    #[test]
    fn delivery_flag_after_a_lowercase_shrinking_char_parses() {
        // `ẞ` is 3 bytes but lowercases to the 2-byte `ß`: a lowercase-derived index
        // used to slice the original text mid-character.
        assert_eq!(
            consume_trailing_delivery_flag("ẞẞ ping --deliver steer").unwrap(),
            Some((DeliveryMode::Steer, "ẞẞ ping".to_string()))
        );
        assert_eq!(
            consume_trailing_delivery_flag("ẞẞ ping --steer").unwrap(),
            Some((DeliveryMode::Steer, "ẞẞ ping".to_string()))
        );
    }
}
