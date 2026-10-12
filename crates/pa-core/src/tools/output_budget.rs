//! Failure-aware output budgets for cell results and kernel streams
//! (fabric `output-budget.ts`): oversized output keeps its head AND tail
//! under a middle omission marker, and the full content spills to a private
//! artifact the text links. A failed cell gets a tight 20k budget so the
//! error and the most recent output stay visible; a successful cell keeps
//! its normal budget. Character counts are Unicode scalar values, matching
//! the kernel manager's per-stream caps.

use std::io::Write as _;

/// The model-visible budget for a failed cell's combined text (fabric
/// `MAX_FAILURE_MODEL_OUTPUT_CHARS`).
pub const MAX_FAILURE_MODEL_OUTPUT_CHARS: usize = 20_000;

/// The bounded result of [`bound_model_output`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundedModelOutput {
    /// The model-visible text, at most `max_chars` characters.
    pub text: String,
    /// The artifact holding the full output, when the spill write
    /// succeeded.
    pub artifact_path: Option<String>,
    /// The full output's character count.
    pub original_chars: usize,
    /// How much of the full output the visible text omits.
    pub omitted_chars: usize,
}

/// The byte index of the `n`-th character (the whole string when `n`
/// covers it), always on a UTF-8 boundary.
fn char_end(value: &str, n: usize) -> usize {
    value.char_indices().nth(n).map_or(value.len(), |(i, _)| i)
}

/// The first `n` characters of `value`.
fn char_prefix(value: &str, n: usize) -> &str {
    &value[..char_end(value, n)]
}

/// The last `n` characters of `value`.
fn char_suffix(value: &str, n: usize) -> &str {
    let total = value.chars().count();
    &value[char_end(value, total.saturating_sub(n))..]
}

/// The middle-omission marker naming the product and the count.
fn middle_marker(omitted: usize) -> String {
    format!("\n\n... {omitted} characters omitted by Prime Agent ...\n\n")
}

/// Middle truncation (fabric `truncateMiddle`): keep the head and the
/// tail, marking the omitted middle. The marker names the product, so
/// the model knows who cut the text and by how much.
#[must_use]
pub fn truncate_middle(value: &str, max_chars: usize) -> String {
    let total = value.chars().count();
    if total <= max_chars {
        return value.to_string();
    }
    let marker = middle_marker(total - max_chars);
    let available = max_chars.saturating_sub(marker.chars().count());
    let head = available.div_ceil(2);
    let tail = available - head;
    format!(
        "{}{}{}",
        char_prefix(value, head),
        marker,
        char_suffix(value, tail)
    )
}

/// The directory-name prefix every output artifact lives under.
const OUTPUT_ARTIFACT_DIR_PREFIX: &str = "prime-agent-output-";

/// How long an output artifact is kept: the model reads it during the
/// session that produced it; a week covers any resumed session.
const OUTPUT_ARTIFACT_MAX_AGE: std::time::Duration = std::time::Duration::from_hours(7 * 24);

/// Remove the output-artifact directories directly under `root` that are
/// older than `max_age` (by modification time). Best-effort: errors are
/// ignored, only real directories named with the artifact prefix are
/// touched, and symlinks are never followed.
pub(crate) fn sweep_stale_output_artifacts(root: &std::path::Path, max_age: std::time::Duration) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    let now = std::time::SystemTime::now();
    for entry in entries.flatten() {
        if !entry
            .file_name()
            .to_string_lossy()
            .starts_with(OUTPUT_ARTIFACT_DIR_PREFIX)
        {
            continue;
        }
        // `DirEntry::metadata` does not follow symlinks.
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if !metadata.is_dir() {
            continue;
        }
        let stale = metadata
            .modified()
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age > max_age);
        if stale {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// Create a fresh private output artifact (fabric `writeOutputArtifact`):
/// a new file under a one-off directory in the system temp dir, created
/// with 0600 permissions and never overwriting. Returns the open handle
/// so stream spills can append incrementally. The first creation in a
/// process sweeps week-old artifact directories, so they never pile up.
pub fn create_output_artifact() -> std::io::Result<(std::fs::File, String)> {
    static SWEEP: std::sync::Once = std::sync::Once::new();
    SWEEP.call_once(|| {
        sweep_stale_output_artifacts(&std::env::temp_dir(), OUTPUT_ARTIFACT_MAX_AGE);
    });
    let directory = std::env::temp_dir().join(format!(
        "{OUTPUT_ARTIFACT_DIR_PREFIX}{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir(&directory)?;
    let path = directory.join("output.txt");
    // Read+write: stream spills append per frame and read the tail
    // back at settle (`StreamSpill::read_tail`).
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)?;
    set_private_permissions(&file)?;
    Ok((file, path.display().to_string()))
}

/// Write the full output to a fresh private artifact (fabric
/// `writeOutputArtifact`).
pub fn write_output_artifact(content: &str) -> std::io::Result<String> {
    let (mut file, path) = create_output_artifact()?;
    file.write_all(content.as_bytes())?;
    Ok(path)
}

/// Best-effort 0600 on POSIX; a no-op elsewhere (fabric asserts 0o600
/// on non-Windows platforms only).
#[cfg(unix)]
fn set_private_permissions(file: &std::fs::File) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(std::fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn set_private_permissions(_file: &std::fs::File) -> std::io::Result<()> {
    Ok(())
}

/// The artifact link appended to bounded text: always the last thing in
/// the text, so the model ends on where the full output lives.
fn artifact_suffix(total_chars: usize, path: &str) -> String {
    format!("\n\n[Full output ({total_chars} chars) saved to: {path}]")
}

/// Bound model-visible text (fabric `boundModelOutput`): keep the head
/// and tail within `max_chars`, spill the full output through
/// `write_artifact`, and link the artifact. When `visible` and
/// `full_output` both fit, the text passes through untouched and no
/// artifact is written. A failed artifact write still bounds the
/// text; it just links nothing.
pub fn bound_model_output(
    visible: &str,
    max_chars: usize,
    full_output: &str,
    write_artifact: impl FnOnce(&str) -> std::io::Result<String>,
) -> BoundedModelOutput {
    let full_chars = full_output.chars().count();
    let visible_chars = visible.chars().count();
    if visible_chars <= max_chars && full_chars <= max_chars {
        return BoundedModelOutput {
            text: visible.to_string(),
            artifact_path: None,
            original_chars: full_chars,
            omitted_chars: 0,
        };
    }

    let artifact_path = write_artifact(full_output).ok();
    let suffix = artifact_path
        .as_ref()
        .map(|path| artifact_suffix(full_chars, path))
        .unwrap_or_default();
    let body_budget = max_chars.saturating_sub(suffix.chars().count()).max(1);
    let body = truncate_middle(visible, body_budget);
    let mut text = format!("{body}{suffix}");
    if text.chars().count() > max_chars {
        // The suffix carries the artifact path; shrink the body again
        // instead of cutting into the path. A truncation marker can
        // itself exceed a tiny rebudget, so fall back to the bare
        // suffix (or its tail), which always fits and still ends with
        // the path (fabric's fallback, verbatim).
        let rebudget = max_chars.saturating_sub(suffix.chars().count());
        if rebudget == 0 {
            let tail = suffix.chars().count().saturating_sub(max_chars);
            text = char_suffix(&suffix, suffix.chars().count() - tail).to_string();
        } else {
            let shrunk = truncate_middle(&body, rebudget);
            text = if shrunk.chars().count() + suffix.chars().count() <= max_chars {
                format!("{shrunk}{suffix}")
            } else {
                suffix
            };
        }
    }
    BoundedModelOutput {
        text,
        artifact_path,
        original_chars: full_chars,
        omitted_chars: full_chars.saturating_sub(full_chars.min(body_budget)),
    }
}

/// Rebuild a truncated kernel stream's visible text: `head` is the
/// capped prefix captured while streaming, `tail` the last chunk read
/// back from the spill artifact, `total_chars` the full stream's
/// character count. Splits the body budget symmetrically between head
/// and tail around the omission marker, with the artifact link always
/// ending the text (fabric `output-budget.ts`).
#[must_use]
pub fn bound_stream_text(
    head: &str,
    tail: &str,
    total_chars: usize,
    artifact_path: &str,
    max_chars: usize,
) -> String {
    let suffix = artifact_suffix(total_chars, artifact_path);
    let suffix_chars = suffix.chars().count();
    let body_budget = max_chars.saturating_sub(suffix_chars).max(1);
    // The marker's width depends on the omitted count, which depends on
    // how much is kept: estimate from the worst case (the whole body
    // budget kept), then hand the digit difference back to the tail.
    let marker_estimate = middle_marker(total_chars.saturating_sub(body_budget));
    let estimate_chars = marker_estimate.chars().count();
    let head_kept = char_prefix(
        head,
        (body_budget.saturating_sub(estimate_chars) / 2).min(head.chars().count()),
    );
    let head_chars = head_kept.chars().count();
    let tail_budget = body_budget
        .saturating_sub(head_chars)
        .saturating_sub(estimate_chars);
    let tail_kept = char_suffix(tail, tail_budget.min(tail.chars().count()));
    let tail_chars = tail_kept.chars().count();
    let marker = middle_marker(total_chars.saturating_sub(head_chars + tail_chars));
    let marker_chars = marker.chars().count();
    let tail_final = if marker_chars < estimate_chars {
        char_suffix(
            tail,
            (tail_chars + (estimate_chars - marker_chars)).min(tail.chars().count()),
        )
    } else {
        char_suffix(
            tail_kept,
            tail_chars.saturating_sub(marker_chars - estimate_chars),
        )
    };
    format!("{head_kept}{marker}{tail_final}{suffix}")
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use super::*;

    #[test]
    fn the_sweep_removes_only_stale_artifact_directories() {
        let root = tempfile::tempdir().unwrap();
        let week = std::time::Duration::from_hours(7 * 24);
        let make = |name: &str, age: std::time::Duration| {
            let dir = root.path().join(name);
            std::fs::create_dir(&dir).unwrap();
            std::fs::write(dir.join("output.txt"), "x").unwrap();
            let modified = std::time::SystemTime::now() - age;
            std::fs::File::open(&dir)
                .unwrap()
                .set_modified(modified)
                .unwrap();
            dir
        };
        let stale = make("prime-agent-output-old", week * 2);
        let fresh = make("prime-agent-output-new", std::time::Duration::from_secs(60));
        let unrelated = make("someone-elses-dir", week * 2);
        sweep_stale_output_artifacts(root.path(), week);
        assert!(!stale.exists(), "a stale artifact directory is removed");
        assert!(fresh.exists(), "a fresh artifact directory is kept");
        assert!(unrelated.exists(), "other directories are never touched");
    }

    #[test]
    fn truncate_middle_keeps_head_and_tail() {
        let value = format!("start-{}-end", "x".repeat(4_000));
        let bounded = truncate_middle(&value, 1_000);
        assert!(
            bounded.chars().count() <= 1_000,
            "{}",
            bounded.chars().count()
        );
        assert!(bounded.starts_with("start-"));
        assert!(bounded.ends_with("-end"));
        assert!(bounded.contains("characters omitted by Prime Agent"));
        let marker_omitted = 4_010
            - (1_000
                - "\n\n... 0 characters omitted by Prime Agent ...\n\n"
                    .chars()
                    .count()
                + 15);
        let _ = marker_omitted;
    }

    #[test]
    fn truncate_middle_leaves_short_values_untouched() {
        assert_eq!(truncate_middle("small", 1_000), "small");
    }

    #[test]
    fn truncate_middle_below_marker_size_returns_the_marker() {
        // A budget smaller than the marker itself: the marker is the
        // whole output (fabric's `truncateMiddle` behaves the same; the
        // final clamp lives in `bound_model_output`).
        let bounded = truncate_middle(&"x".repeat(500), 40);
        assert!(bounded.contains("characters omitted by Prime Agent"));
        assert!(bounded.chars().count() <= 60);
    }

    #[test]
    fn bound_model_output_leaves_output_under_budget_untouched() {
        let writes = Arc::new(AtomicUsize::new(0));
        let probe = Arc::clone(&writes);
        let bounded = bound_model_output("small", 1_000, "small", move |_| {
            probe.fetch_add(1, Ordering::SeqCst);
            Ok("/never".to_string())
        });
        assert_eq!(
            bounded,
            BoundedModelOutput {
                text: "small".to_string(),
                artifact_path: None,
                original_chars: 5,
                omitted_chars: 0,
            }
        );
        assert_eq!(writes.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn bound_model_output_bounds_and_links_the_artifact() {
        let full = format!("start-{}-end", "x".repeat(4_000));
        let writer = |content: &str| {
            assert_eq!(content, full);
            Ok("/tmp/prime-agent-output/output.txt".to_string())
        };
        let bounded = bound_model_output(&full, 1_000, &full, writer);
        assert!(bounded.text.chars().count() <= 1_000);
        assert!(bounded.text.contains("start-"));
        assert!(bounded.text.contains("-end"));
        assert!(bounded
            .text
            .contains("Full output (4010 chars) saved to: /tmp/prime-agent-output/output.txt"));
        assert!(bounded.text.ends_with(']'));
        assert_eq!(
            bounded.artifact_path.as_deref(),
            Some("/tmp/prime-agent-output/output.txt")
        );
        assert!(bounded.omitted_chars > 0);
        assert_eq!(bounded.original_chars, 4_010);
    }

    #[test]
    fn bound_model_output_never_cuts_the_artifact_path() {
        let full = "x".repeat(4_000);
        let path = "/tmp/prime-agent-output/output.txt";
        let bounded = bound_model_output(&full, 100, &full, move |_| Ok(path.to_string()));
        assert!(bounded.text.chars().count() <= 100);
        assert!(bounded.text.contains(path));
        assert_eq!(bounded.artifact_path.as_deref(), Some(path));
        assert!(bounded.omitted_chars > 0);
    }

    #[test]
    fn bound_model_output_survives_artifact_write_failure() {
        let full = "x".repeat(4_000);
        let bounded = bound_model_output(&full, 1_000, &full, |_| {
            Err(std::io::Error::other("disk full"))
        });
        assert!(bounded.text.chars().count() <= 1_000);
        assert_eq!(bounded.artifact_path, None);
        assert!(bounded.text.contains("characters omitted by Prime Agent"));
        assert!(bounded.omitted_chars > 0);
    }

    #[test]
    fn write_output_artifact_persists_private_content() {
        let content = "full output";
        let path = write_output_artifact(content).expect("scratch write");
        let persisted = std::fs::read_to_string(&path).expect("read back");
        assert_eq!(persisted, content);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).expect("stat").permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        std::fs::remove_file(&path).ok();
        std::fs::remove_dir(std::path::Path::new(&path).parent().expect("dir")).ok();
    }

    #[test]
    fn bound_stream_text_keeps_the_head_and_the_last_line() {
        let head = "h".repeat(60_000);
        let tail = "\nthe last line survives";
        let bounded = bound_stream_text(&head, tail, 150_000, "/tmp/spill/output.txt", 65_536);
        assert!(bounded.chars().count() <= 65_536);
        assert!(bounded.starts_with("hhhhh"));
        assert!(bounded.ends_with("[Full output (150000 chars) saved to: /tmp/spill/output.txt]"));
        assert!(bounded.contains("the last line survives"));
        assert!(bounded.contains("characters omitted by Prime Agent"));
    }
}
