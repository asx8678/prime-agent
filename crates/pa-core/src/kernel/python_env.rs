//! Python path hygiene for every host site that runs the kernel Python
//! inside a user project directory.
//!
//! [`manager::startup`] builds the kernel environment; the bootstrap probe
//! and the CLI's factory dispatch run the same interpreter with `-c`. All
//! of them must stop a project's own `rlm` package from shadowing the
//! installed runtime — without leaking that protection into the commands
//! the agent runs through `bash()` (a leaked PYTHONSAFEPATH breaks
//! `python3 script.py` sibling imports, `-m`, and pytest's cwd imports).

/// Marker the host sets next to the PYTHONSAFEPATH it injects. The runtime
/// pops the pair at the very start of `rlm.repl.main()` before any cell or
/// `bash()` child can inherit them, so a PYTHONSAFEPATH the user set
/// themselves (no marker) survives.
pub const HOST_SAFEPATH_MARKER_ENV: &str = "PRIME_AGENT_KERNEL_SAFEPATH";

/// The original PYTHONPATH the host records next to the entries it strips
/// from the kernel's environment. The runtime pops it at the very start of
/// `rlm.repl.main()` and restores (or, for the sentinel, removes) PYTHONPATH
/// before any cell or `bash()` child can inherit the stripped value: the
/// strip protects the runtime's own imports, not the agent's commands.
/// Always written when the host applies its protection, so a stale inherited
/// value can never survive into a fresh kernel.
pub const HOST_PYTHONPATH_ENV: &str = "PRIME_AGENT_KERNEL_PYTHONPATH";

/// Sentinel recorded in [`HOST_PYTHONPATH_ENV`] when PYTHONPATH was unset:
/// a leading `\u{1}` cannot occur in a real path list (Windows forbids
/// control characters in filenames; on every platform it is not a path
/// anyone sets). Must stay byte-identical to `_HOST_PYTHONPATH_UNSET` in
/// `prime-agent-runtime/src/rlm/repl.py`.
pub const HOST_PYTHONPATH_UNSET: &str = "\u{1}";

/// Drop empty PYTHONPATH entries: the common `export PYTHONPATH=$PYTHONPATH:/x`
/// yields ":/x", and an empty entry resolves to the cwd at import time —
/// putting the project directory back on `sys.path` ahead of the installed
/// runtime, which re-enables the shadowing PYTHONSAFEPATH exists to stop.
/// Real entries stay joined in their original order.
#[must_use]
pub fn strip_empty_pythonpath_entries(value: &str) -> String {
    let separator = if cfg!(windows) { ';' } else { ':' };
    value
        .split(separator)
        .filter(|entry| !entry.is_empty())
        .collect::<Vec<_>>()
        .join(&separator.to_string())
}

/// Apply the user-project import protection to a command about to run the
/// kernel Python with `-c` in a user project directory: PYTHONSAFEPATH stops
/// the interpreter from prepending the process cwd to `sys.path`, and the
/// inherited PYTHONPATH is stripped of its empty entries (which resolve to
/// the cwd at import time and would re-enable that shadowing). Used by the
/// runtime-ready probe and the CLI's factory dispatch.
pub fn protect_user_project_imports(command: &mut std::process::Command) {
    command.env("PYTHONSAFEPATH", "1");
    if let Some(pythonpath) = std::env::var_os("PYTHONPATH") {
        let stripped = strip_empty_pythonpath_entries(&pythonpath.to_string_lossy());
        if stripped.is_empty() {
            command.env_remove("PYTHONPATH");
        } else {
            command.env("PYTHONPATH", stripped);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_leading_trailing_and_inner_empty_entries() {
        assert_eq!(strip_empty_pythonpath_entries(":/x"), "/x");
        assert_eq!(strip_empty_pythonpath_entries("/x:"), "/x");
        assert_eq!(strip_empty_pythonpath_entries(":/a::/b:"), "/a:/b");
        assert_eq!(strip_empty_pythonpath_entries(":"), "");
        assert_eq!(strip_empty_pythonpath_entries(""), "");
        assert_eq!(strip_empty_pythonpath_entries("/a:/b"), "/a:/b");
    }
}
