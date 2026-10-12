//! The `ipython` tool: persistent Python REPL execution through a kernel —
//! the model-facing definition, result text composition, busy-kernel choice,
//! and the rlm bootstrap code. Kernel process management lives behind the
//! [`IpythonKernelProvisioner`] trait, owned by the kernel manager module.

use std::fmt::Write as _;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use serde_json::json;

use crate::tools::tool_definition::{
    AbortSignal, ExecutionMode, OnUpdate, ToolContentBlock, ToolDefinition, ToolExecutionResult,
    ToolUpdate,
};

/// Mime types the model context accepts as images.
pub const IMAGE_MIME_TYPES: [&str; 4] = ["image/jpeg", "image/png", "image/gif", "image/webp"];

// Kernel execution types

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct KernelErrorInfo {
    pub ename: String,
    pub evalue: String,
    pub traceback: Vec<String>,
}

/// A media attachment loaded into context (e.g. by the attach-image skill).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KernelAttachment {
    pub mime_type: String,
    pub data: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ExecuteStatus {
    #[default]
    Ok,
    Error,
    Aborted,
}

#[derive(Debug, Clone, Default)]
pub struct ExecuteResult {
    pub status: ExecuteStatus,
    pub stdout: String,
    pub stderr: String,
    pub result: Option<String>,
    pub duration_ms: Option<u64>,
    /// Output that arrived without this cell's id, shown separately.
    pub background_output: Option<String>,
    pub error: Option<KernelErrorInfo>,
    pub attachments: Vec<KernelAttachment>,
    /// Files edited from this cell, in order (`diffs` on the tool-result
    /// details).
    pub diffs: Vec<crate::kernel::shared::KernelDiffDisplay>,
    /// Agent messages sent from this cell, in order (TS
    /// `sentAgentMessages` on the tool-result details).
    pub sent_agent_messages: Vec<crate::kernel::shared::KernelSentAgentMessage>,
    /// The `bash()` commands this cell started, summarized for display
    /// (`bashCommands` on the tool-result details).
    pub bash_commands: Option<crate::kernel::shared::KernelBashCommands>,
    /// The context-fold marker the cell emitted, raw (`contextFold` on the
    /// tool-result details).
    pub context_fold: Option<serde_json::Value>,
    /// Where the full text of each output that overflowed its kernel
    /// budget lives (the visible text holds only its head and tail).
    pub spilled: crate::kernel::shared::SpilledOutputs,
}

/// The most of one spilled output a failed cell's full-output artifact
/// reads back (64 MiB); a larger output is cut there with a marker.
const MAX_SPILL_READ_BYTES: u64 = 64 * 1024 * 1024;

/// The wire form of one sent agent message (TS `KernelSentAgentMessage`):
/// `id`, `message`, `deliveryStatus`, `receiverRole` when present, and the
/// `target` endpoint (`sessionName` only when present).
#[must_use]
pub fn sent_agent_message_json(
    sent: &crate::kernel::shared::KernelSentAgentMessage,
) -> serde_json::Value {
    use crate::kernel::shared::{SentAgentMessageTarget, SentDeliveryStatus};
    let crate::kernel::shared::KernelSentAgentMessage {
        id,
        message,
        delivery_status,
        receiver_role,
        target:
            SentAgentMessageTarget {
                active_session_id,
                session_id,
                session_name,
            },
    } = sent;
    let delivery = match delivery_status {
        SentDeliveryStatus::Delivered => "delivered",
        SentDeliveryStatus::Queued => "queued",
    };
    let mut value = json!({
        "id": id,
        "message": message,
        "deliveryStatus": delivery,
        "target": {
            "activeSessionId": active_session_id,
            "sessionId": session_id,
        },
    });
    if let Some(role) = receiver_role {
        value["receiverRole"] = json!(role.as_str());
    }
    if let Some(name) = session_name {
        value["target"]["sessionName"] = json!(name);
    }
    value
}

/// The kernel is still running a previously interrupted cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KernelBusyAfterInterruptError {
    pub message: String,
}

impl std::fmt::Display for KernelBusyAfterInterruptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl Default for KernelBusyAfterInterruptError {
    fn default() -> Self {
        Self {
            message: "The Python kernel is still running the previously interrupted cell. Wait and try again, or kill the kernel to start fresh.".to_string(),
        }
    }
}

#[derive(Debug)]
pub enum KernelExecError {
    /// The kernel is busy with a previously interrupted cell.
    BusyAfterInterrupt(KernelBusyAfterInterruptError),
    /// Any other kernel failure.
    Other(anyhow::Error),
}

impl KernelExecError {
    /// True when this error is a busy-after-interrupt error.
    #[must_use]
    pub fn is_busy_after_interrupt(&self) -> bool {
        matches!(self, KernelExecError::BusyAfterInterrupt(_))
    }

    /// The model-facing message.
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            KernelExecError::BusyAfterInterrupt(err) => err.message.clone(),
            // The full context chain, not just the outermost layer: a bare
            // top-level message hides the actual cause of kernel failures.
            KernelExecError::Other(err) => format!("{err:#}"),
        }
    }
}

pub type StreamFn<'a> = Option<&'a (dyn Fn(&str, &'static str) + Send + Sync)>;

pub type LateSentAgentMessageHandler =
    std::sync::Arc<dyn Fn(&str, crate::kernel::shared::KernelSentAgentMessage) + Send + Sync>;

pub struct KernelExecuteOptions<'a> {
    pub signal: Option<AbortSignal>,
    /// Streams cell output while the cell runs.
    pub on_stream: StreamFn<'a>,
    pub on_late_sent_agent_message: Option<crate::kernel::shared::LateSentAgentMessageCallback>,
}

type ExecuteCellFuture =
    Pin<Box<dyn Future<Output = Result<ExecuteResult, KernelExecError>> + Send>>;

/// A running kernel able to execute code cells.
pub trait KernelExecutor: Send + Sync {
    /// Execute one cell in the persistent kernel.
    fn execute(&self, code: &str, options: KernelExecuteOptions<'_>) -> ExecuteCellFuture;
}

/// Startup progress handler (TS: `KernelBootstrapProgressHandler`).
pub type BootstrapProgressHandler = Arc<dyn Fn(&str) + Send + Sync>;

type EnsureFuture = Pin<Box<dyn Future<Output = anyhow::Result<Box<dyn KernelExecutor>>> + Send>>;

/// Owns the lazy create+start+bootstrap of one session's Python kernel.
/// Implementations memoize one running kernel (concurrent `ensure` calls await
/// the same in-flight startup; a failed startup clears the memo), and `kill`
/// terminates the kernel losing all in-memory state. Object-safe on purpose.
pub trait IpythonKernelProvisioner: Send + Sync {
    /// Start (or reuse) the kernel; resolves once it is ready to execute.
    fn ensure(
        &self,
        on_progress: Option<BootstrapProgressHandler>,
        signal: Option<AbortSignal>,
    ) -> EnsureFuture;

    /// Kill the kernel immediately, losing all in-memory state.
    fn kill(&self) -> Pin<Box<dyn Future<Output = ()> + Send>>;
}

// Busy-kernel choice UI

pub const BUSY_KERNEL_WAIT_CHOICE: &str = "Wait and preserve state";
pub const BUSY_KERNEL_KILL_CHOICE: &str = "Kill kernel and restart";

pub fn busy_kernel_prompt() -> String {
    [
        "Interrupted Python cell is still running",
        "Ctrl+C sent an interrupt, but the previous cell has not stopped yet. A new command cannot start until it finishes.",
        "Waiting preserves the current kernel state. Killing restarts the kernel and loses in-memory variables, imports, and running tasks.",
    ]
    .join("\n")
}

pub fn kernel_restart_notice() -> &'static str {
    "<ipython_kernel_reset>\nThe Python kernel was restarted after a previous interrupted cell kept running. Variables, imports, async tasks, and open resources from before the restart are no longer available; recreate them before using them.\n</ipython_kernel_reset>"
}

/// The UI surface the ipython tool needs from the host session (the TS tool context).
pub trait IpythonToolUi: Send + Sync {
    /// Prompt the user to choose between `choices`.
    fn select(
        &self,
        prompt: &str,
        choices: &[&str],
        signal: Option<&AbortSignal>,
    ) -> Pin<Box<dyn Future<Output = Option<String>> + Send>>;

    /// Show/hide a transient working message.
    fn set_working_message(&self, message: Option<&str>);
}

// Tool definition

/// Turn kernel image attachments into image blocks; non-images are dropped.
pub fn image_blocks_from_attachments(attachments: &[KernelAttachment]) -> Vec<ToolContentBlock> {
    attachments
        .iter()
        .filter(|a| IMAGE_MIME_TYPES.contains(&a.mime_type.as_str()))
        .map(|a| ToolContentBlock::Image {
            data: a.data.clone(),
            mime_type: a.mime_type.clone(),
        })
        .collect()
}

fn format_execute_text(result: &ExecuteResult, background_output: Option<&str>) -> String {
    let mut text = result.stdout.clone();
    if !result.stderr.is_empty() {
        text.push_str(if text.is_empty() { "" } else { "\n" });
        text.push_str(&result.stderr);
    }
    if let Some(result_text) = &result.result {
        text.push_str(if text.is_empty() { "" } else { "\n" });
        text.push_str(result_text);
    }
    if result.status == ExecuteStatus::Error {
        if let Some(error) = &result.error {
            text.push_str(if text.is_empty() { "" } else { "\n" });
            text.push_str(&error.traceback.join("\n"));
        }
    }
    if let Some(background) = background_output {
        let separator = if text.is_empty() { "" } else { "\n" };
        let _ = write!(
            text,
            "{separator}[background output (unattributed)]\n{background}"
        );
    }
    text
}

/// The most edited paths the failure note lists (fabric
/// `MAX_COMPLETED_CALLS`).
const MAX_FAILURE_NOTE_PATHS: usize = 8;
/// The per-path character budget in the failure note (fabric
/// `MAX_PATH_CHARS`).
const MAX_FAILURE_NOTE_PATH_CHARS: usize = 100;

/// The workspace-side-effects note appended to a failed cell's text
/// (fabric `failure-progress.ts`): the edits and bash commands that already
/// ran before the failure, bounded, plus the inspect-before-repeating line.
/// `None` for successful cells and failures without side effects.
fn failure_progress_note(result: &ExecuteResult) -> Option<String> {
    if result.status != ExecuteStatus::Error {
        return None;
    }
    let bash_count = result
        .bash_commands
        .as_ref()
        .map(|bash| bash.count)
        .filter(|count| *count > 0);
    if result.diffs.is_empty() && bash_count.is_none() {
        return None;
    }
    let mut items: Vec<String> = result
        .diffs
        .iter()
        .take(MAX_FAILURE_NOTE_PATHS)
        .map(|diff| format!("edit({})", compact_failure_note_path(&diff.path)))
        .collect();
    let omitted = result.diffs.len().saturating_sub(items.len());
    if omitted > 0 {
        items.push(format!("+{omitted} more"));
    }
    if let Some(count) = bash_count {
        items.push(if count == 1 {
            "1 bash command".to_string()
        } else {
            format!("{count} bash commands")
        });
    }
    Some(format!(
        "Completed before the cell failed (outputs not returned): {}.\nSuccessful calls may already have changed the workspace; inspect before repeating mutations.",
        items.join("; ")
    ))
}

/// Collapse whitespace and keep the tail within the path budget (fabric
/// `compactPath`).
fn compact_failure_note_path(value: &str) -> String {
    let single_line = value.split_whitespace().collect::<Vec<_>>().join(" ");
    let chars = single_line.chars().count();
    if chars <= MAX_FAILURE_NOTE_PATH_CHARS {
        return single_line;
    }
    let tail: String = single_line
        .chars()
        .skip(chars - (MAX_FAILURE_NOTE_PATH_CHARS - 1))
        .collect();
    format!("…{tail}")
}

async fn execute_with_busy_kernel_choice(
    provisioner: &dyn IpythonKernelProvisioner,
    report_startup_progress: &BootstrapProgressHandler,
    code: &str,
    execute: KernelExecuteOptions<'_>,
    on_working_message: &(dyn Fn(Option<&str>) + Send + Sync),
    ui: Option<&Arc<dyn IpythonToolUi>>,
) -> Result<(ExecuteResult, bool), KernelExecError> {
    let mut kernel_restarted = false;
    loop {
        let manager = provisioner
            .ensure(
                Some(report_startup_progress.clone()),
                execute.signal.clone(),
            )
            .await
            .map_err(KernelExecError::Other)?;
        let result = manager
            .execute(
                code,
                KernelExecuteOptions {
                    signal: execute.signal.clone(),
                    on_stream: execute.on_stream,
                    on_late_sent_agent_message: execute.on_late_sent_agent_message.clone(),
                },
            )
            .await;
        match result {
            Ok(result) => return Ok((result, kernel_restarted)),
            Err(err) => {
                let aborted = execute
                    .signal
                    .as_ref()
                    .is_some_and(tokio_util::sync::CancellationToken::is_cancelled);
                if !err.is_busy_after_interrupt() || aborted {
                    return Err(err);
                }
                // No UI (headless): cancel immediately.
                let Some(ui) = ui else {
                    return Err(err);
                };
                let choice = ui
                    .select(
                        &busy_kernel_prompt(),
                        &[BUSY_KERNEL_WAIT_CHOICE, BUSY_KERNEL_KILL_CHOICE],
                        execute.signal.as_ref(),
                    )
                    .await;
                match choice.as_deref() {
                    Some(BUSY_KERNEL_WAIT_CHOICE) => {
                        on_working_message(Some("Waiting for Python kernel..."));
                    }
                    Some(BUSY_KERNEL_KILL_CHOICE) => {
                        on_working_message(Some("Restarting Python kernel..."));
                        provisioner.kill().await;
                        kernel_restarted = true;
                    }
                    _ => return Err(err),
                }
            }
        }
    }
}

pub struct IpythonToolOptions {
    /// Shared provisioner owning the kernel lifecycle.
    pub provisioner: Arc<dyn IpythonKernelProvisioner>,
    /// UI surface; `None` in headless sessions.
    pub ui: Option<Arc<dyn IpythonToolUi>>,
    pub on_late_sent_agent_message: Option<LateSentAgentMessageHandler>,
    /// The repeat-guard configuration; `None` disables the guard entirely
    /// (the golden-replay harness and parity mode).
    pub repeat_guard: Option<RepeatGuardConfig>,
    /// Adoption telemetry for the repeat guard: called with whether the
    /// observation blocked and the identical-run count (primitives only).
    pub on_repeat_guard_triggered: Option<RepeatGuardTriggeredHandler>,
    /// Adoption telemetry for the failure-progress note: called when a
    /// failed cell's result carries the workspace-side-effects note.
    pub on_cell_failure_note: Option<CellFailureNoteHandler>,
    /// Adoption telemetry for the output budget: called when a failed
    /// cell's combined text was bounded and its full output spilled to a
    /// linked artifact.
    pub on_output_budget_spill: Option<OutputBudgetSpillHandler>,
}

/// The repeat-guard thresholds (fabric `repeat-guard.ts` defaults: warn 3,
/// block 6 — the degenerate loop that motivated them was a 220× identical
/// `bash echo noop` run on a low-tier model).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RepeatGuardConfig {
    /// Consecutive identical runs (same code, same output) that start
    /// producing a warning.
    pub warn_at: u64,
    /// Consecutive identical runs after which the same code is blocked
    /// outright.
    pub block_at: u64,
}

impl RepeatGuardConfig {
    /// The default-on configuration from `repeatGuard` settings
    /// (`repeatGuard.enabled: false` resolves to `None`). Thresholds must
    /// satisfy `1 <= warnAt < blockAt`; an invalid pair falls back to the
    /// defaults (3/6) with a warning, never to a guard that blocks every
    /// call.
    #[must_use]
    pub fn from_settings(settings: &crate::settings::RepeatGuardSettings) -> Option<Self> {
        if !settings.enabled.unwrap_or(true) {
            return None;
        }
        let warn_at = settings.warn_at.unwrap_or(IPYTHON_REPEAT_WARN_AT);
        let block_at = settings.block_at.unwrap_or(IPYTHON_REPEAT_BLOCK_AT);
        if warn_at >= 1 && block_at > warn_at {
            return Some(Self { warn_at, block_at });
        }
        eprintln!(
            "Warning: repeatGuard thresholds need 1 <= warnAt < blockAt (got warnAt {warn_at}, blockAt {block_at}); using the defaults ({IPYTHON_REPEAT_WARN_AT}/{IPYTHON_REPEAT_BLOCK_AT})."
        );
        Some(Self {
            warn_at: IPYTHON_REPEAT_WARN_AT,
            block_at: IPYTHON_REPEAT_BLOCK_AT,
        })
    }
}

/// Consecutive identical runs that start producing a warning (fabric
/// `FABRIC_REPEAT_WARN`).
pub const IPYTHON_REPEAT_WARN_AT: u64 = 3;
/// Consecutive identical runs that get blocked outright (fabric
/// `FABRIC_REPEAT_BLOCK`).
pub const IPYTHON_REPEAT_BLOCK_AT: u64 = 6;

/// The model-facing warning appended to a warned run's result (fabric
/// `fabricRepeatWarnText`, adapted to the `ipython` tool and to the
/// identical-output rule).
#[must_use]
pub fn repeat_warn_text(count: u64, block_at: u64) -> String {
    format!(
        "[circuit-breaker] identical ipython code has now run {count} times in a row with identical output; at {block_at} identical runs it will be blocked. Change the approach or finish your turn."
    )
}

/// The model-facing text of a blocked run (fabric `fabricRepeatBlockText`,
/// adapted to the `ipython` tool and to the identical-output rule).
#[must_use]
pub fn repeat_block_text(count: u64) -> String {
    format!(
        "Circuit breaker: this exact ipython code would run for the {count}th time in a row after producing identical output every time, so it was blocked. Re-running it adds no information. Summarize what you already know and finish your turn; if the call is genuinely required, change the code materially."
    )
}

/// One observation of the repeat guard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RepeatGuardObservation {
    /// The consecutive-identical run length including this call.
    pub count: u64,
    /// This run reached the warn threshold (`count >= warnAt`).
    pub warn: bool,
    /// This call is blocked before it runs.
    pub blocked: bool,
}

/// Degenerate-loop breaker for the ipython tool (fabric
/// `FabricRepeatGuard`, Rust-side refinement): a run counts toward the
/// loop only when its code AND its result are identical to the previous
/// run's — re-running the same cell with nothing changing is how lower-tier
/// models burn a session, while polling a cell whose output moves (an
/// `rlm.collect` while children work) is legitimate. Differing code or a
/// differing result resets the count to 1.
pub struct IpythonRepeatGuard {
    last_code: Option<String>,
    last_output: Option<u64>,
    count: u64,
    config: RepeatGuardConfig,
}

impl IpythonRepeatGuard {
    #[must_use]
    pub fn new(config: RepeatGuardConfig) -> Self {
        Self {
            last_code: None,
            last_output: None,
            count: 0,
            config,
        }
    }

    /// The thresholds this guard enforces.
    #[must_use]
    pub fn config(&self) -> RepeatGuardConfig {
        self.config
    }

    /// Before a run: `Some` (blocked) when `code` repeats a run that has
    /// already produced the same output `blockAt - 1` times in a row. A
    /// blocked call leaves the state unchanged, so the same code stays
    /// blocked and any other code proceeds.
    #[must_use]
    pub fn check(&self, code: &str) -> Option<RepeatGuardObservation> {
        let repeats = self.last_code.as_deref() == Some(code)
            && self.count.saturating_add(1) >= self.config.block_at;
        repeats.then(|| RepeatGuardObservation {
            count: self.count.saturating_add(1),
            warn: true,
            blocked: true,
        })
    }

    /// After a run: fold the run's code and result into the count. The
    /// result compares by a fingerprint of its model-visible text and
    /// error flag (before any warning is appended).
    pub fn record(&mut self, code: &str, result: &ToolExecutionResult) -> RepeatGuardObservation {
        use std::hash::{Hash as _, Hasher as _};
        let mut hasher = std::hash::DefaultHasher::new();
        result.is_error.hash(&mut hasher);
        for block in &result.content {
            match block {
                ToolContentBlock::Text { text } => text.hash(&mut hasher),
                ToolContentBlock::Image { data, mime_type } => {
                    data.hash(&mut hasher);
                    mime_type.hash(&mut hasher);
                }
            }
        }
        let output = hasher.finish();
        self.count = if self.last_code.as_deref() == Some(code) && self.last_output == Some(output)
        {
            self.count.saturating_add(1)
        } else {
            1
        };
        self.last_code = Some(code.to_string());
        self.last_output = Some(output);
        RepeatGuardObservation {
            count: self.count,
            warn: self.count >= self.config.warn_at,
            blocked: false,
        }
    }

    /// Forget the run (a call that failed before producing a result has
    /// nothing to compare).
    pub fn reset(&mut self) {
        self.last_code = None;
        self.last_output = None;
        self.count = 0;
    }
}

/// The repeat-guard telemetry seam: `(blocked, count)`, primitives only.
pub type RepeatGuardTriggeredHandler = Arc<dyn Fn(bool, u64) + Send + Sync>;

/// The failure-progress-note telemetry seam (no payload: the note's paths
/// and commands stay local; only the count uploads).
pub type CellFailureNoteHandler = Arc<dyn Fn() + Send + Sync>;

/// The output-budget telemetry seam: fires when a failed cell's text was
/// bounded and its full output spilled to a linked artifact (no payload:
/// only the count uploads).
pub type OutputBudgetSpillHandler = Arc<dyn Fn() + Send + Sync>;

pub fn ipython_tool_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "required": ["code"],
        "properties": {
            "code": {
                "type": "string",
                "description": "Python code to execute in the persistent Python REPL. Use the target project's own environment for project imports, tests, scripts, CLIs, and dependency checks instead of direct kernel imports."
            }
        }
    })
}

pub fn ipython_tool_description() -> &'static str {
    "Execute Python code in a persistent Python REPL. Top-level `await` is supported. Variables, imports, and loaded data persist across calls, and are revived on a best-effort basis when a session is resumed (objects that cannot be serialized are dropped and reported). Run shell commands with `bash('cmd')` / `await bash('cmd')`. Project imports, tests, scripts, CLIs, and dependency checks should run through the target project's own environment."
}

#[tracing::instrument(
    level = "debug",
    name = "tool_ipython_execute",
    skip(options, on_update, on_late_sent_agent_message)
    fields(code),
)]
pub async fn execute_ipython(
    options: &IpythonToolOptions,
    code: &str,
    signal: Option<AbortSignal>,
    on_update: Option<OnUpdate>,
    on_late_sent_agent_message: Option<crate::kernel::shared::LateSentAgentMessageCallback>,
) -> anyhow::Result<ToolExecutionResult> {
    let set_tool_working_message = |message: Option<&str>| {
        if let Some(ui) = &options.ui {
            ui.set_working_message(message);
        }
    };

    let ui = options.ui.clone();
    let report_startup_progress: BootstrapProgressHandler = {
        let on_update = on_update.clone();
        let ui = ui.clone();
        Arc::new(move |message: &str| {
            if let Some(ui) = &ui {
                ui.set_working_message(Some(message));
            }
            if let Some(on_update) = &on_update {
                on_update(ToolUpdate {
                    content: vec![ToolContentBlock::text(message.to_string())],
                    details: Some(json!({ "status": "starting" })),
                });
            }
        })
    };

    // Stream cell output chunks as in-flight updates (TS onStream).
    let stream_update = |chunk: &str, _stream: &'static str| {
        if let Some(on_update) = &on_update {
            on_update(ToolUpdate {
                content: vec![ToolContentBlock::text(chunk.to_string())],
                details: Some(json!({ "status": "ok" })),
            });
        }
    };

    let result = execute_with_busy_kernel_choice(
        options.provisioner.as_ref(),
        &report_startup_progress,
        code,
        KernelExecuteOptions {
            signal,
            on_stream: Some(&stream_update),
            on_late_sent_agent_message,
        },
        &|message| {
            set_tool_working_message(message);
        },
        options.ui.as_ref(),
    )
    .await;

    set_tool_working_message(None);

    let (r, kernel_restarted) = result.map_err(|err| anyhow::anyhow!("{}", err.message()))?;

    let mut text = format_execute_text(&r, r.background_output.as_deref());
    // A failed cell that already changed the workspace carries the
    // side-effects note so the model inspects before repeating mutations
    // (fabric `failure-progress.ts`); successful cells stay unchanged.
    if let Some(note) = failure_progress_note(&r) {
        if let Some(on_note) = &options.on_cell_failure_note {
            on_note();
        }
        text = if text.is_empty() {
            note
        } else {
            format!("{text}\n\n{note}")
        };
    }
    if kernel_restarted {
        text = if text.is_empty() {
            kernel_restart_notice().to_string()
        } else {
            format!("{}\n\n{}", kernel_restart_notice(), text)
        };
    }
    // Failure-aware output budget (fabric `output-budget.ts`): a failed
    // cell's combined text is capped at 20k with middle truncation, its
    // full output spilled to a private artifact the text links — the
    // error and the most recent output stay visible instead of a
    // head-only cut. Successful cells keep the per-stream budgets from
    // the kernel manager; aborted cells keep today's text too.
    if r.status == ExecuteStatus::Error {
        // An output that already overflowed its kernel budget shows only
        // its head and tail in `text`: the artifact gets the complete
        // output, read back from the spill files, so it really is the full
        // output and its character count is the true one.
        let full_output = if r.spilled == crate::kernel::shared::SpilledOutputs::default() {
            None
        } else {
            let read_back = |path: &Option<String>, visible: &str| -> String {
                use std::io::Read as _;
                let Some(path) = path else {
                    return visible.to_string();
                };
                let mut bytes = Vec::new();
                let read = std::fs::File::open(path)
                    .and_then(|file| file.take(MAX_SPILL_READ_BYTES + 1).read_to_end(&mut bytes));
                match read {
                    Ok(_) if bytes.len() as u64 > MAX_SPILL_READ_BYTES => {
                        bytes.truncate(usize::try_from(MAX_SPILL_READ_BYTES).unwrap_or(usize::MAX));
                        format!(
                            "{}\n[... cut at {MAX_SPILL_READ_BYTES} bytes; the rest is in {path} ...]",
                            String::from_utf8_lossy(&bytes)
                        )
                    }
                    Ok(_) => String::from_utf8_lossy(&bytes).into_owned(),
                    // A vanished or unreadable spill keeps the visible text.
                    Err(_) => visible.to_string(),
                }
            };
            let full = ExecuteResult {
                stdout: read_back(&r.spilled.stdout, &r.stdout),
                stderr: read_back(&r.spilled.stderr, &r.stderr),
                result: r
                    .result
                    .as_ref()
                    .map(|visible| read_back(&r.spilled.result, visible)),
                ..r.clone()
            };
            Some(format_execute_text(
                &full,
                full.background_output.as_deref(),
            ))
        };
        let bounded = super::output_budget::bound_model_output(
            &text,
            super::output_budget::MAX_FAILURE_MODEL_OUTPUT_CHARS,
            full_output.as_deref().unwrap_or(&text),
            super::output_budget::write_output_artifact,
        );
        if bounded.artifact_path.is_some() {
            if let Some(on_spill) = &options.on_output_budget_spill {
                on_spill();
            }
        }
        text = bounded.text;
    }

    let image_blocks = image_blocks_from_attachments(&r.attachments);
    let mut content = vec![ToolContentBlock::text(text)];
    content.extend(image_blocks);

    let mut details = json!({
        "status": match r.status {
            ExecuteStatus::Ok => "ok",
            ExecuteStatus::Error => "error",
            ExecuteStatus::Aborted => "aborted",
        },
        "kernelRestarted": kernel_restarted,
    });
    if let Some(duration) = r.duration_ms {
        details["durationMs"] = json!(duration);
    }
    if !r.stdout.is_empty() {
        details["stdout"] = json!(r.stdout);
    }
    if !r.stderr.is_empty() {
        details["stderr"] = json!(r.stderr);
    }
    if let Some(result_text) = &r.result {
        details["result"] = json!(result_text);
    }
    if let Some(bash) = &r.bash_commands {
        details["bashCommands"] = json!({
            "first": bash.first,
            "count": bash.count,
            "lines": bash.lines,
        });
    }
    if let Some(background) = &r.background_output {
        details["backgroundOutput"] = json!(background);
    }
    if let Some(error) = &r.error {
        details["error"] = json!({
            "ename": error.ename,
            "evalue": error.evalue,
            "traceback": error.traceback,
        });
        details["errorEname"] = json!(error.ename);
    }
    if !r.diffs.is_empty() {
        details["diffs"] = json!(r
            .diffs
            .iter()
            .map(|diff| {
                let mut entry = json!({
                    "path": diff.path,
                    "oldStr": diff.old_str,
                    "newStr": diff.new_str,
                });
                if let Some(start_line) = diff.start_line {
                    entry["startLine"] = json!(start_line);
                }
                entry
            })
            .collect::<Vec<_>>());
    }
    if !r.sent_agent_messages.is_empty() {
        details["sentAgentMessages"] = json!(r
            .sent_agent_messages
            .iter()
            .map(sent_agent_message_json)
            .collect::<Vec<_>>());
    }
    if let Some(marker) = &r.context_fold {
        details["contextFold"] = marker.clone();
    }

    Ok(ToolExecutionResult {
        content,
        details: Some(details),
        is_error: r.status == ExecuteStatus::Error || r.status == ExecuteStatus::Aborted,
    })
}

/// The `ipython` tool definition: exact name, schema, and description.
///
/// # Panics
///
/// Panics if the repeat-guard state mutex is poisoned (a prior call
/// panicked while holding it).
#[must_use]
pub fn create_ipython_tool_definition(_cwd: &str, options: IpythonToolOptions) -> ToolDefinition {
    let options = Arc::new(options);
    let execute: crate::tools::tool_definition::ExecuteFn = {
        let options = options;
        // One guard per tool instance (per session): the count survives every
        // call of this session's kernel (fabric `repeat-guard.ts`).
        let repeat_guard = options
            .repeat_guard
            .map(|config| Arc::new(Mutex::new(IpythonRepeatGuard::new(config))));
        Arc::new(move |tool_call_id, params, signal, on_update| {
            let options = options.clone();
            let repeat_guard = repeat_guard.clone();
            let on_late_sent_agent_message =
                options.on_late_sent_agent_message.as_ref().map(|handler| {
                    let handler = std::sync::Arc::clone(handler);
                    let tool_call_id = tool_call_id.to_string();
                    std::sync::Arc::new(move |message| handler(&tool_call_id, message))
                        as crate::kernel::shared::LateSentAgentMessageCallback
                });
            Box::pin(async move {
                let code = params
                    .get("code")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| anyhow::anyhow!("ipython tool requires a code string"))?
                    .to_string();
                // The repeat guard: a blocked call never reaches the kernel;
                // a warned call runs and carries the circuit-breaker line.
                if let Some(guard) = &repeat_guard {
                    let blocked = guard.lock().expect("repeat guard poisoned").check(&code);
                    if let Some(observation) = blocked {
                        if let Some(on_triggered) = &options.on_repeat_guard_triggered {
                            on_triggered(true, observation.count);
                        }
                        return Ok(ToolExecutionResult {
                            content: vec![ToolContentBlock::text(repeat_block_text(
                                observation.count,
                            ))],
                            details: Some(json!({
                                "status": "error",
                                "repeatGuard": { "blocked": true, "count": observation.count },
                            })),
                            is_error: true,
                        });
                    }
                }
                let executed = execute_ipython(
                    &options,
                    &code,
                    signal,
                    on_update,
                    on_late_sent_agent_message,
                )
                .await;
                let mut result = match executed {
                    Ok(result) => result,
                    Err(error) => {
                        if let Some(guard) = &repeat_guard {
                            guard.lock().expect("repeat guard poisoned").reset();
                        }
                        return Err(error);
                    }
                };
                if let Some(guard) = &repeat_guard {
                    let (observation, config) = {
                        let mut guard = guard.lock().expect("repeat guard poisoned");
                        (guard.record(&code, &result), guard.config())
                    };
                    if observation.count == config.warn_at {
                        if let Some(on_triggered) = &options.on_repeat_guard_triggered {
                            on_triggered(false, observation.count);
                        }
                    }
                    if observation.warn {
                        let warn = repeat_warn_text(observation.count, config.block_at);
                        match result.content.last_mut() {
                            Some(ToolContentBlock::Text { text }) => {
                                if text.is_empty() {
                                    text.push_str(&warn);
                                } else {
                                    text.push_str("\n\n");
                                    text.push_str(&warn);
                                }
                            }
                            Some(ToolContentBlock::Image { .. }) | None => {
                                result.content.push(ToolContentBlock::text(warn));
                            }
                        }
                    }
                }
                Ok(result)
            })
        })
    };
    ToolDefinition {
        name: "ipython".to_string(),
        label: "ipython".to_string(),
        description: ipython_tool_description().to_string(),
        prompt_snippet:
            "ipython - persistent Python REPL for code, state, and bash() orchestration".to_string(),
        // The kernel is single-threaded; calls must not run in parallel.
        execution_mode: Some(ExecutionMode::Sequential),
        parameters: ipython_tool_schema(),
        prepare_arguments: None,
        execute,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::shared::KernelDiffDisplay;

    /// A kernel whose every cell settles with the given result.
    struct SettledCell(ExecuteResult);

    impl KernelExecutor for SettledCell {
        fn execute(&self, _code: &str, _options: KernelExecuteOptions<'_>) -> ExecuteCellFuture {
            Box::pin(std::future::ready(Ok(self.0.clone())))
        }
    }

    impl IpythonKernelProvisioner for SettledCell {
        fn ensure(
            &self,
            _on_progress: Option<BootstrapProgressHandler>,
            _signal: Option<AbortSignal>,
        ) -> EnsureFuture {
            let executor: Box<dyn KernelExecutor> = Box::new(SettledCell(self.0.clone()));
            Box::pin(std::future::ready(Ok(executor)))
        }

        fn kill(&self) -> Pin<Box<dyn Future<Output = ()> + Send>> {
            Box::pin(std::future::ready(()))
        }
    }

    /// A settled kernel whose cell executions are counted: proves the
    /// repeat guard blocks *before* the executor runs.
    struct CountedSettledCell {
        cell: ExecuteResult,
        executions: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl KernelExecutor for CountedSettledCell {
        fn execute(&self, _code: &str, _options: KernelExecuteOptions<'_>) -> ExecuteCellFuture {
            self.executions
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let cell = self.cell.clone();
            Box::pin(std::future::ready(Ok(cell)))
        }
    }

    impl IpythonKernelProvisioner for CountedSettledCell {
        fn ensure(
            &self,
            _on_progress: Option<BootstrapProgressHandler>,
            _signal: Option<AbortSignal>,
        ) -> EnsureFuture {
            let executor: Box<dyn KernelExecutor> = Box::new(CountedSettledCell {
                cell: self.cell.clone(),
                executions: self.executions.clone(),
            });
            Box::pin(std::future::ready(Ok(executor)))
        }

        fn kill(&self) -> Pin<Box<dyn Future<Output = ()> + Send>> {
            Box::pin(std::future::ready(()))
        }
    }

    /// Run one call through a tool definition with the given code.
    async fn run_code(
        definition: &ToolDefinition,
        code: &str,
    ) -> anyhow::Result<ToolExecutionResult> {
        (definition.execute)("call-1", json!({ "code": code }), None, None).await
    }

    fn guarded_options(
        provisioner: Arc<dyn IpythonKernelProvisioner>,
        on_repeat_guard_triggered: Option<crate::tools::ipython::RepeatGuardTriggeredHandler>,
    ) -> IpythonToolOptions {
        IpythonToolOptions {
            provisioner,
            ui: None,
            on_late_sent_agent_message: None,
            repeat_guard: Some(RepeatGuardConfig {
                warn_at: IPYTHON_REPEAT_WARN_AT,
                block_at: IPYTHON_REPEAT_BLOCK_AT,
            }),
            on_repeat_guard_triggered,
            on_cell_failure_note: None,
            on_output_budget_spill: None,
        }
    }

    fn text_result(text: &str) -> ToolExecutionResult {
        ToolExecutionResult {
            content: vec![ToolContentBlock::text(text)],
            details: None,
            is_error: false,
        }
    }

    #[test]
    fn repeat_guard_counts_identical_code_and_output_and_resets_on_change() {
        let mut guard = IpythonRepeatGuard::new(RepeatGuardConfig {
            warn_at: 3,
            block_at: 6,
        });
        assert_eq!(guard.check("x = 1"), None);
        assert_eq!(
            guard.record("x = 1", &text_result("same")),
            RepeatGuardObservation {
                count: 1,
                warn: false,
                blocked: false,
            }
        );
        guard.record("x = 1", &text_result("same"));
        assert_eq!(
            guard.record("x = 1", &text_result("same")),
            RepeatGuardObservation {
                count: 3,
                warn: true,
                blocked: false,
            }
        );
        // The same code with a moving output is a poll, not a loop.
        assert_eq!(guard.record("x = 1", &text_result("moved")).count, 1);
        // Different code resets too.
        assert_eq!(guard.record("y = 2", &text_result("moved")).count, 1);
        for _ in 0..4 {
            guard.record("y = 2", &text_result("moved"));
        }
        assert_eq!(
            guard.check("y = 2"),
            Some(RepeatGuardObservation {
                count: 6,
                warn: true,
                blocked: true,
            })
        );
        assert_eq!(guard.check("z = 3"), None, "other code is never blocked");
        guard.reset();
        assert_eq!(guard.check("y = 2"), None, "a reset forgets the run");
    }

    #[test]
    fn repeat_guard_invalid_thresholds_fall_back_to_the_defaults() {
        let defaults = Some(RepeatGuardConfig {
            warn_at: IPYTHON_REPEAT_WARN_AT,
            block_at: IPYTHON_REPEAT_BLOCK_AT,
        });
        for (warn_at, block_at) in [(Some(0), Some(6)), (Some(4), Some(4)), (Some(7), None)] {
            let settings = crate::settings::RepeatGuardSettings {
                enabled: None,
                warn_at,
                block_at,
            };
            assert_eq!(
                RepeatGuardConfig::from_settings(&settings),
                defaults,
                "{warn_at:?}/{block_at:?}"
            );
        }
        let custom = crate::settings::RepeatGuardSettings {
            enabled: Some(true),
            warn_at: Some(2),
            block_at: Some(4),
        };
        assert_eq!(
            RepeatGuardConfig::from_settings(&custom),
            Some(RepeatGuardConfig {
                warn_at: 2,
                block_at: 4,
            })
        );
        let disabled = crate::settings::RepeatGuardSettings {
            enabled: Some(false),
            warn_at: None,
            block_at: None,
        };
        assert_eq!(RepeatGuardConfig::from_settings(&disabled), None);
    }

    /// A kernel whose every cell prints a new line: the same code polling a
    /// moving state.
    struct PollingCell {
        executions: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl KernelExecutor for PollingCell {
        fn execute(&self, _code: &str, _options: KernelExecuteOptions<'_>) -> ExecuteCellFuture {
            let run = self
                .executions
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Box::pin(std::future::ready(Ok(ExecuteResult {
                stdout: format!("children still running (poll {run})"),
                ..ExecuteResult::default()
            })))
        }
    }

    impl IpythonKernelProvisioner for PollingCell {
        fn ensure(
            &self,
            _on_progress: Option<BootstrapProgressHandler>,
            _signal: Option<AbortSignal>,
        ) -> EnsureFuture {
            let executor: Box<dyn KernelExecutor> = Box::new(PollingCell {
                executions: self.executions.clone(),
            });
            Box::pin(std::future::ready(Ok(executor)))
        }

        fn kill(&self) -> Pin<Box<dyn Future<Output = ()> + Send>> {
            Box::pin(std::future::ready(()))
        }
    }

    /// Re-running the same polling cell while its output changes is never
    /// warned or blocked (the `rlm.collect` while children work case).
    #[tokio::test]
    async fn repeat_guard_never_trips_on_a_poll_whose_output_moves() {
        let executions = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let definition = create_ipython_tool_definition(
            "/tmp",
            guarded_options(
                Arc::new(PollingCell {
                    executions: executions.clone(),
                }),
                None,
            ),
        );
        for poll in 0..10 {
            let result = run_code(&definition, "await rlm.collect()")
                .await
                .expect("the poll settles");
            assert_eq!(
                result.content,
                vec![ToolContentBlock::text(format!(
                    "children still running (poll {poll})"
                ))]
            );
        }
        assert_eq!(executions.load(std::sync::atomic::Ordering::SeqCst), 10);
    }

    #[tokio::test]
    async fn repeat_guard_third_identical_call_warns() {
        let cell = ExecuteResult {
            stdout: "settled".to_string(),
            ..ExecuteResult::default()
        };
        let definition = create_ipython_tool_definition(
            "/tmp",
            guarded_options(Arc::new(SettledCell(cell)), None),
        );
        let first = run_code(&definition, "x = 1").await.expect("first settles");
        assert_eq!(first.content, vec![ToolContentBlock::text("settled")]);
        run_code(&definition, "x = 1")
            .await
            .expect("second settles");
        let third = run_code(&definition, "x = 1").await.expect("third settles");
        let expected = format!(
            "settled\n\n{}",
            repeat_warn_text(IPYTHON_REPEAT_WARN_AT, IPYTHON_REPEAT_BLOCK_AT)
        );
        assert_eq!(third.content, vec![ToolContentBlock::text(expected)]);
        assert!(!third.is_error, "a warned run still executed");
    }

    #[tokio::test]
    async fn repeat_guard_sixth_identical_call_is_blocked_without_execution() {
        let cell = ExecuteResult {
            stdout: "settled".to_string(),
            ..ExecuteResult::default()
        };
        let executions = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed: std::sync::Arc<std::sync::Mutex<Vec<(bool, u64)>>> = Arc::default();
        let recorder = observed.clone();
        let telemetry: crate::tools::ipython::RepeatGuardTriggeredHandler =
            Arc::new(move |blocked: bool, count: u64| {
                recorder.lock().unwrap().push((blocked, count));
            });
        let definition = create_ipython_tool_definition(
            "/tmp",
            guarded_options(
                Arc::new(CountedSettledCell {
                    cell,
                    executions: executions.clone(),
                }),
                Some(telemetry),
            ),
        );
        for _ in 0..5 {
            run_code(&definition, "x = 1")
                .await
                .expect("the run settles");
        }
        assert_eq!(
            executions.load(std::sync::atomic::Ordering::SeqCst),
            5,
            "the first five calls executed"
        );
        let sixth = run_code(&definition, "x = 1")
            .await
            .expect("the block returns");
        assert_eq!(
            sixth.content,
            vec![ToolContentBlock::text(repeat_block_text(
                IPYTHON_REPEAT_BLOCK_AT
            ))]
        );
        assert!(sixth.is_error, "the blocked call is an error result");
        assert_eq!(
            sixth.details,
            Some(json!({
                "status": "error",
                "repeatGuard": { "blocked": true, "count": IPYTHON_REPEAT_BLOCK_AT },
            }))
        );
        assert_eq!(
            executions.load(std::sync::atomic::Ordering::SeqCst),
            5,
            "the blocked call never reached the executor"
        );
        assert_eq!(
            observed.lock().unwrap().as_slice(),
            [
                (false, IPYTHON_REPEAT_WARN_AT),
                (true, IPYTHON_REPEAT_BLOCK_AT)
            ],
            "telemetry fires on the warn crossing and every block"
        );
    }

    #[tokio::test]
    async fn repeat_guard_changed_code_resets_the_run() {
        let cell = ExecuteResult {
            stdout: "settled".to_string(),
            ..ExecuteResult::default()
        };
        let definition = create_ipython_tool_definition(
            "/tmp",
            guarded_options(Arc::new(SettledCell(cell)), None),
        );
        for _ in 0..3 {
            run_code(&definition, "x = 1")
                .await
                .expect("the run settles");
        }
        // Differing code resets; the next identical run restarts at 1.
        run_code(&definition, "y = 2")
            .await
            .expect("the reset settles");
        let after_reset = run_code(&definition, "x = 1").await.expect("settles");
        assert_eq!(
            after_reset.content,
            vec![ToolContentBlock::text("settled")],
            "the count restarted at 1 after the differing code"
        );
    }

    #[tokio::test]
    async fn repeat_guard_disabled_runs_identical_code_freely() {
        let cell = ExecuteResult {
            stdout: "settled".to_string(),
            ..ExecuteResult::default()
        };
        let executions = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let options = IpythonToolOptions {
            provisioner: Arc::new(CountedSettledCell {
                cell,
                executions: executions.clone(),
            }),
            ui: None,
            on_late_sent_agent_message: None,
            repeat_guard: None,
            on_repeat_guard_triggered: None,
            on_cell_failure_note: None,
            on_output_budget_spill: None,
        };
        let definition = create_ipython_tool_definition("/tmp", options);
        for _ in 0..8 {
            let result = run_code(&definition, "x = 1").await.expect("settles");
            assert_eq!(result.content, vec![ToolContentBlock::text("settled")]);
        }
        assert_eq!(
            executions.load(std::sync::atomic::Ordering::SeqCst),
            8,
            "without the guard every identical call executes"
        );
    }

    #[tokio::test]
    async fn failed_cell_with_side_effects_appends_the_progress_note() {
        let cell = ExecuteResult {
            status: ExecuteStatus::Error,
            stdout: "partial out".to_string(),
            error: Some(KernelErrorInfo {
                ename: "RuntimeError".to_string(),
                evalue: "boom".to_string(),
                traceback: vec![
                    "Traceback line 1".to_string(),
                    "Traceback line 2".to_string(),
                ],
            }),
            diffs: vec![
                KernelDiffDisplay {
                    path: "/repo/a.py".to_string(),
                    old_str: "a = 1".to_string(),
                    new_str: "a = 2".to_string(),
                    start_line: None,
                },
                KernelDiffDisplay {
                    path: "/repo/b.py".to_string(),
                    old_str: "x".to_string(),
                    new_str: "y".to_string(),
                    start_line: None,
                },
            ],
            bash_commands: Some(crate::kernel::shared::KernelBashCommands {
                first: "cargo test".to_string(),
                count: 1,
                lines: 1,
            }),
            ..ExecuteResult::default()
        };
        let noted = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let recorder = noted.clone();
        let options = IpythonToolOptions {
            provisioner: Arc::new(SettledCell(cell)),
            ui: None,
            on_late_sent_agent_message: None,
            repeat_guard: None,
            on_repeat_guard_triggered: None,
            on_cell_failure_note: Some(Arc::new(move || {
                recorder.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            })),
            on_output_budget_spill: None,
        };
        let result = execute_ipython(&options, "await edit(...)", None, None, None)
            .await
            .expect("the failed cell settles");
        assert_eq!(
            result.content,
            vec![ToolContentBlock::text(
                "partial out\nTraceback line 1\nTraceback line 2\n\nCompleted before the cell failed (outputs not returned): edit(/repo/a.py); edit(/repo/b.py); 1 bash command.\nSuccessful calls may already have changed the workspace; inspect before repeating mutations."
            )]
        );
        assert!(result.is_error);
        assert_eq!(
            noted.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "the failure-note telemetry seam fired once"
        );
    }

    #[tokio::test]
    async fn successful_cell_with_side_effects_appends_no_note() {
        let cell = ExecuteResult {
            status: ExecuteStatus::Ok,
            stdout: "done".to_string(),
            diffs: vec![KernelDiffDisplay {
                path: "/repo/a.py".to_string(),
                old_str: "a = 1".to_string(),
                new_str: "a = 2".to_string(),
                start_line: None,
            }],
            bash_commands: Some(crate::kernel::shared::KernelBashCommands {
                first: "cargo test".to_string(),
                count: 2,
                lines: 2,
            }),
            ..ExecuteResult::default()
        };
        let options = IpythonToolOptions {
            provisioner: Arc::new(SettledCell(cell)),
            ui: None,
            on_late_sent_agent_message: None,
            repeat_guard: None,
            on_repeat_guard_triggered: None,
            on_cell_failure_note: None,
            on_output_budget_spill: None,
        };
        let result = execute_ipython(&options, "await edit(...)", None, None, None)
            .await
            .expect("the cell settles");
        assert_eq!(result.content, vec![ToolContentBlock::text("done")]);
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn failed_cell_output_is_bounded_at_20k_with_a_linked_artifact() {
        // The plan's verifier (b): a failing 100k-char cell yields at most
        // 20k of model-visible text ending with the artifact path, and the
        // artifact holds the full output.
        let cell = ExecuteResult {
            status: ExecuteStatus::Error,
            stdout: format!("start-{}-the very last line", "x".repeat(100_000)),
            error: Some(KernelErrorInfo {
                ename: "RuntimeError".to_string(),
                evalue: "boom".to_string(),
                traceback: vec!["Traceback line 1".to_string()],
            }),
            ..ExecuteResult::default()
        };
        let spills = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let spill_counter = Arc::clone(&spills);
        let options = IpythonToolOptions {
            provisioner: Arc::new(SettledCell(cell)),
            ui: None,
            on_late_sent_agent_message: None,
            repeat_guard: None,
            on_repeat_guard_triggered: None,
            on_cell_failure_note: None,
            on_output_budget_spill: Some(Arc::new(move || {
                spill_counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            })),
        };
        let result = execute_ipython(&options, "print('x' * 100_000)", None, None, None)
            .await
            .expect("the failed cell settles");
        assert!(result.is_error);
        let text = match &result.content[0] {
            ToolContentBlock::Text { text } => text.clone(),
            ToolContentBlock::Image { .. } => panic!("expected a text block"),
        };
        assert!(
            text.chars().count() <= super::super::output_budget::MAX_FAILURE_MODEL_OUTPUT_CHARS,
            "{}",
            text.chars().count()
        );
        assert!(text.contains("characters omitted by Prime Agent"));
        assert!(text.contains("saved to: "), "links the artifact: {text}");
        assert!(text.ends_with(']'));
        let artifact_path = text
            .rsplit_once("saved to: ")
            .expect("the link is present")
            .1
            .trim_end_matches(']');
        let persisted = std::fs::read_to_string(artifact_path).expect("the artifact exists");
        // The artifact holds the full combined text: the stdout plus the
        // error traceback, byte for byte.
        assert_eq!(
            persisted,
            format!(
                "start-{}-the very last line\nTraceback line 1",
                "x".repeat(100_000)
            )
        );
        assert_eq!(spills.load(std::sync::atomic::Ordering::SeqCst), 1);
        std::fs::remove_file(artifact_path).ok();
        std::fs::remove_dir(std::path::Path::new(artifact_path).parent().expect("dir")).ok();
    }

    #[tokio::test]
    async fn failed_cell_artifact_holds_the_complete_spilled_stream() {
        // A stream that overflowed its kernel cap reaches the tool as head
        // + tail with a link; the failed-cell artifact must still hold the
        // complete stream, read back from the kernel's spill file, and its
        // character count must be the true one.
        let spill_dir = tempfile::tempdir().unwrap();
        let spill_path = spill_dir.path().join("output.txt");
        let full_stdout = format!(
            "head-{}-middle-{}-tail",
            "a".repeat(60_000),
            "b".repeat(60_000)
        );
        std::fs::write(&spill_path, &full_stdout).unwrap();
        let cell = ExecuteResult {
            status: ExecuteStatus::Error,
            stdout: format!(
                "head-{}\n\n... characters omitted ...\n\n{}-tail",
                "a".repeat(30_000),
                "b".repeat(30_000)
            ),
            error: Some(KernelErrorInfo {
                ename: "RuntimeError".to_string(),
                evalue: "boom".to_string(),
                traceback: vec!["Traceback line 1".to_string()],
            }),
            spilled: crate::kernel::shared::SpilledOutputs {
                stdout: Some(spill_path.display().to_string()),
                stderr: None,
                result: None,
            },
            ..ExecuteResult::default()
        };
        let options = IpythonToolOptions {
            provisioner: Arc::new(SettledCell(cell)),
            ui: None,
            on_late_sent_agent_message: None,
            repeat_guard: None,
            on_repeat_guard_triggered: None,
            on_cell_failure_note: None,
            on_output_budget_spill: None,
        };
        let result = execute_ipython(&options, "print(big)", None, None, None)
            .await
            .expect("the failed cell settles");
        let text = match &result.content[0] {
            ToolContentBlock::Text { text } => text.clone(),
            ToolContentBlock::Image { .. } => panic!("expected a text block"),
        };
        let expected_full = format!("{full_stdout}\nTraceback line 1");
        let (_, link) = text
            .rsplit_once("[Full output (")
            .expect("the link is present");
        let (count, path) = link
            .split_once(" chars) saved to: ")
            .expect("the link shape");
        assert_eq!(count, expected_full.chars().count().to_string());
        let artifact_path = path.trim_end_matches(']');
        assert_eq!(
            std::fs::read_to_string(artifact_path).expect("the artifact exists"),
            expected_full
        );
        std::fs::remove_file(artifact_path).ok();
        std::fs::remove_dir(std::path::Path::new(artifact_path).parent().expect("dir")).ok();
    }

    #[tokio::test]
    async fn successful_cell_keeps_its_full_output() {
        // Success keeps today's budget: no 20k failure cap, no artifact.
        let cell = ExecuteResult {
            stdout: "y".repeat(100_000),
            ..ExecuteResult::default()
        };
        let options = IpythonToolOptions {
            provisioner: Arc::new(SettledCell(cell)),
            ui: None,
            on_late_sent_agent_message: None,
            repeat_guard: None,
            on_repeat_guard_triggered: None,
            on_cell_failure_note: None,
            on_output_budget_spill: None,
        };
        let result = execute_ipython(&options, "print('y' * 100_000)", None, None, None)
            .await
            .expect("the cell settles");
        assert!(!result.is_error);
        assert_eq!(
            result.content,
            vec![ToolContentBlock::text("y".repeat(100_000))]
        );
    }

    #[tokio::test]
    async fn failed_cell_without_side_effects_appends_no_note() {
        let cell = ExecuteResult {
            status: ExecuteStatus::Error,
            stdout: "partial out".to_string(),
            ..ExecuteResult::default()
        };
        let options = IpythonToolOptions {
            provisioner: Arc::new(SettledCell(cell)),
            ui: None,
            on_late_sent_agent_message: None,
            repeat_guard: None,
            on_repeat_guard_triggered: None,
            on_cell_failure_note: None,
            on_output_budget_spill: None,
        };
        let result = execute_ipython(&options, "1 / 0", None, None, None)
            .await
            .expect("the failed cell settles");
        assert_eq!(result.content, vec![ToolContentBlock::text("partial out")]);
    }

    #[tokio::test]
    async fn failed_cell_note_bounds_paths_and_commands() {
        let long_path = "p".repeat(120);
        let diffs = (1..=10)
            .map(|index| KernelDiffDisplay {
                path: if index == 3 {
                    long_path.clone()
                } else {
                    format!("/repo/f{index}.rs")
                },
                old_str: String::new(),
                new_str: String::new(),
                start_line: None,
            })
            .collect::<Vec<_>>();
        let cell = ExecuteResult {
            status: ExecuteStatus::Error,
            diffs,
            bash_commands: Some(crate::kernel::shared::KernelBashCommands {
                first: "cargo test".to_string(),
                count: 3,
                lines: 3,
            }),
            ..ExecuteResult::default()
        };
        let options = IpythonToolOptions {
            provisioner: Arc::new(SettledCell(cell)),
            ui: None,
            on_late_sent_agent_message: None,
            repeat_guard: None,
            on_repeat_guard_triggered: None,
            on_cell_failure_note: None,
            on_output_budget_spill: None,
        };
        let result = execute_ipython(&options, "await edit(...)", None, None, None)
            .await
            .expect("the failed cell settles");
        let compacted = format!("…{}", "p".repeat(99));
        let expected = format!(
            "Completed before the cell failed (outputs not returned): edit(/repo/f1.rs); edit(/repo/f2.rs); edit({compacted}); edit(/repo/f4.rs); edit(/repo/f5.rs); edit(/repo/f6.rs); edit(/repo/f7.rs); edit(/repo/f8.rs); +2 more; 3 bash commands.\nSuccessful calls may already have changed the workspace; inspect before repeating mutations."
        );
        assert_eq!(result.content, vec![ToolContentBlock::text(expected)]);
    }

    #[tokio::test]
    async fn cell_edit_diffs_are_saved_in_the_result_details() {
        let cell = ExecuteResult {
            result: Some("'Edited /repo/b.py'".to_string()),
            duration_ms: Some(4),
            diffs: vec![
                KernelDiffDisplay {
                    path: "/repo/a.py".to_string(),
                    old_str: "a = 1".to_string(),
                    new_str: "a = 2".to_string(),
                    start_line: Some(3),
                },
                KernelDiffDisplay {
                    path: "/repo/b.py".to_string(),
                    old_str: "x".to_string(),
                    new_str: "y".to_string(),
                    start_line: None,
                },
            ],
            ..ExecuteResult::default()
        };
        let options = IpythonToolOptions {
            provisioner: Arc::new(SettledCell(cell)),
            ui: None,
            on_late_sent_agent_message: None,
            repeat_guard: None,
            on_repeat_guard_triggered: None,
            on_cell_failure_note: None,
            on_output_budget_spill: None,
        };
        let result = execute_ipython(&options, "await edit(...)", None, None, None)
            .await
            .expect("the cell settles");
        assert_eq!(
            result.details,
            Some(json!({
                "status": "ok",
                "kernelRestarted": false,
                "durationMs": 4,
                "result": "'Edited /repo/b.py'",
                "diffs": [
                    { "path": "/repo/a.py", "oldStr": "a = 1", "newStr": "a = 2", "startLine": 3 },
                    { "path": "/repo/b.py", "oldStr": "x", "newStr": "y" },
                ],
            }))
        );
    }

    #[tokio::test]
    async fn cell_context_fold_marker_is_saved_in_the_result_details() {
        let marker = json!({ "kind": "begin", "description": "explore", "goal": "find the bug" });
        // A failed cell keeps its marker too: the fold opened before the
        // error, and the session engine decides what it means.
        for (status, label) in [(ExecuteStatus::Ok, "ok"), (ExecuteStatus::Error, "error")] {
            let cell = ExecuteResult {
                status,
                stdout: "Fold opened: explore.\n".to_string(),
                duration_ms: Some(4),
                context_fold: Some(marker.clone()),
                ..ExecuteResult::default()
            };
            let options = IpythonToolOptions {
                provisioner: Arc::new(SettledCell(cell)),
                ui: None,
                on_late_sent_agent_message: None,
                repeat_guard: None,
                on_repeat_guard_triggered: None,
                on_cell_failure_note: None,
                on_output_budget_spill: None,
            };
            let result = execute_ipython(&options, "await fold.begin(...)", None, None, None)
                .await
                .expect("the cell settles");
            assert_eq!(
                result.details,
                Some(json!({
                    "status": label,
                    "kernelRestarted": false,
                    "durationMs": 4,
                    "stdout": "Fold opened: explore.\n",
                    "contextFold": marker,
                })),
                "{label}"
            );
        }
    }

    #[test]
    fn sent_agent_message_json_matches_ts_wire_shape() {
        let sent = crate::kernel::shared::KernelSentAgentMessage {
            id: "agentmsg_1".to_string(),
            message: "Ping.\nThen report back.".to_string(),
            delivery_status: crate::kernel::shared::SentDeliveryStatus::Delivered,
            receiver_role: Some(crate::kernel::shared::ReceiverRole::Parent),
            target: crate::kernel::shared::SentAgentMessageTarget {
                active_session_id: "worker-active".to_string(),
                session_id: "worker-session".to_string(),
                session_name: Some("Worker".to_string()),
            },
        };
        assert_eq!(
            sent_agent_message_json(&sent),
            json!({
                "id": "agentmsg_1",
                "message": "Ping.\nThen report back.",
                "deliveryStatus": "delivered",
                "receiverRole": "parent",
                "target": {
                    "activeSessionId": "worker-active",
                    "sessionId": "worker-session",
                    "sessionName": "Worker",
                },
            })
        );
        // The queued receipt without a role or session name omits both.
        let queued = crate::kernel::shared::KernelSentAgentMessage {
            id: "agentmsg_2".to_string(),
            message: "Ping.".to_string(),
            delivery_status: crate::kernel::shared::SentDeliveryStatus::Queued,
            receiver_role: None,
            target: crate::kernel::shared::SentAgentMessageTarget {
                active_session_id: "a1".to_string(),
                session_id: "s1".to_string(),
                session_name: None,
            },
        };
        assert_eq!(
            sent_agent_message_json(&queued),
            json!({
                "id": "agentmsg_2",
                "message": "Ping.",
                "deliveryStatus": "queued",
                "target": { "activeSessionId": "a1", "sessionId": "s1" },
            })
        );
    }
}
