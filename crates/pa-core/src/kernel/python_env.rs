//! Python path hygiene for every host site that runs the kernel Python
//! inside a user project directory.
//!
//! [`manager::startup`] builds the kernel environment; the bootstrap probe
//! and the CLI's factory dispatch run the same interpreter with `-c`. All
//! of them must stop a project's own `rlm` package from shadowing the
//! installed runtime — without leaking that protection into the commands
//! the agent runs through `bash()` (a leaked PYTHONSAFEPATH breaks
//! `python3 script.py` sibling imports, `-m`, and pytest's cwd imports).
//!
//! The strip drops every PYTHONPATH entry that resolves to the child's
//! working directory, not just literal empty ones: `.`, `./`, `sub/..`
//! and the absolute cwd path all put the project directory back on
//! `sys.path` at import time. An entry pointing at a project *subdirectory*
//! that contains an `rlm/` package (for example `PYTHONPATH=src`) can
//! still shadow the runtime — detecting that would mean inspecting
//! project contents, which is out of scope here.
//!
//! The module is crate-internal except for
//! [`protect_user_project_imports`], which pa-cli's factory dispatch
//! needs (re-exported at the kernel facade).

/// Marker the host sets next to the PYTHONSAFEPATH it injects. The runtime
/// pops the pair at the very start of `rlm.repl.main()` before any cell or
/// `bash()` child can inherit them, so a PYTHONSAFEPATH the user set
/// themselves (no marker) survives.
pub(crate) const HOST_SAFEPATH_MARKER_ENV: &str = "PRIME_AGENT_KERNEL_SAFEPATH";

/// The original PYTHONPATH the host records next to the entries it strips
/// from the kernel's environment. The runtime pops it at the very start of
/// `rlm.repl.main()` and restores (or, for the sentinel, removes) PYTHONPATH
/// before any cell or `bash()` child can inherit the stripped value: the
/// strip protects the runtime's own imports, not the agent's commands.
/// Always written when the host applies its protection, so a stale inherited
/// value can never survive into a fresh kernel.
pub(crate) const HOST_PYTHONPATH_ENV: &str = "PRIME_AGENT_KERNEL_PYTHONPATH";

/// Sentinel recorded in [`HOST_PYTHONPATH_ENV`] when PYTHONPATH was unset:
/// a leading `\u{1}` cannot occur in a real path list (Windows forbids
/// control characters in filenames; on every platform it is not a path
/// anyone sets). Must stay byte-identical to `_HOST_PYTHONPATH_UNSET` in
/// `prime-agent-runtime/src/rlm/repl.py`.
pub(crate) const HOST_PYTHONPATH_UNSET: &str = "\u{1}";

/// Drop PYTHONPATH entries that resolve to the child's working directory:
/// the common `export PYTHONPATH=$PYTHONPATH:/x` yields ":/x", and an
/// empty entry resolves to the cwd at import time — as do `.`, `./`,
/// `sub/..` and the absolute cwd path — putting the project directory
/// back on `sys.path` ahead of the installed runtime, which re-enables
/// the shadowing PYTHONSAFEPATH exists to stop. Real entries stay joined
/// in their original order. `None` means every entry was dropped and the
/// variable must be removed entirely.
///
/// Out of scope: an entry pointing at a project subdirectory that contains
/// an `rlm/` package (for example `PYTHONPATH=src`) still shadows the
/// runtime; detecting that would mean inspecting project contents, not
/// path hygiene.
#[must_use]
pub(crate) fn strip_cwd_resolving_pythonpath_entries(
    value: &std::ffi::OsStr,
    cwd: Option<&std::path::Path>,
) -> Option<std::ffi::OsString> {
    let kept: Vec<std::path::PathBuf> = std::env::split_paths(value)
        .filter(|entry| {
            !entry.as_os_str().is_empty()
                && !cwd.is_some_and(|cwd| entry_resolves_to_cwd(entry, cwd))
        })
        .collect();
    // join_paths fails only when a kept entry itself contains the platform
    // separator (possible on Unix): such a value cannot be expressed as a
    // PYTHONPATH at all, so drop the whole variable rather than risk
    // re-enabling the shadowing. An empty join (every entry dropped) is the
    // same case: PYTHONPATH="" is not "unset" to Python — it adds the cwd
    // back — so the variable must be removed entirely.
    match std::env::join_paths(kept) {
        Ok(joined) if !joined.is_empty() => Some(joined),
        _ => None,
    }
}

/// True when a PYTHONPATH entry resolves to `cwd`: literally, through
/// `.`, `./` or `sub/..`, or as the absolute path. Existing paths compare
/// canonically (symlinks included); the lexical fallback covers paths
/// that do not exist yet, where the OS cannot canonicalize.
fn entry_resolves_to_cwd(entry: &std::path::Path, cwd: &std::path::Path) -> bool {
    let joined = if entry.is_absolute() {
        entry.to_path_buf()
    } else {
        cwd.join(entry)
    };
    let cwd_identities = path_identities(cwd);
    path_identities(&joined)
        .iter()
        .any(|identity| cwd_identities.contains(identity))
}

/// The identities a path can compare under: the canonical form when the
/// path exists, and the lexically normalized absolute form (`.`, `..` and
/// duplicate separators collapsed without resolving symlinks).
fn path_identities(path: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut identities = Vec::new();
    if let Ok(canon) = std::fs::canonicalize(path) {
        identities.push(canon);
    }
    identities.push(lexical_absolute(path));
    identities
}

/// The absolute form of a path with `.`, `..` and duplicate separators
/// collapsed, purely lexically: no filesystem access, no symlink
/// resolution (`std::path::absolute` already normalizes on Windows; on
/// Unix it does not, so the component walk does it).
fn lexical_absolute(path: &std::path::Path) -> std::path::PathBuf {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut normalized = std::path::PathBuf::new();
    for component in absolute.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

/// Apply the kernel-start import protection to the environment map the
/// kernel child receives — the single implementation behind
/// `manager::startup`'s spawn (the `-c` sites use
/// [`protect_user_project_imports`], sharing the strip below):
///
/// * `PYTHONSAFEPATH=1` is injected with the [`HOST_SAFEPATH_MARKER_ENV`]
///   marker unless the user set a non-empty value themselves (an empty
///   value means "unset" to Python, so it still gets the protection); the
///   user branch drops any inherited marker so the runtime can never pop
///   the user's own setting.
/// * `PYTHONPATH` entries that resolve to the kernel's cwd are stripped,
///   and the original is recorded in [`HOST_PYTHONPATH_ENV`] (the
///   [`HOST_PYTHONPATH_UNSET`] sentinel when it was unset) so the runtime
///   restores it before any cell or `bash()` child can inherit the
///   stripped value. Recording always happens so a stale inherited value
///   can never survive into a fresh kernel.
pub(crate) fn protect_kernel_env(
    env: &mut std::collections::HashMap<String, String>,
    cwd: Option<&std::path::Path>,
) {
    if env
        .get("PYTHONSAFEPATH")
        .is_some_and(|value| !value.is_empty())
    {
        env.remove(HOST_SAFEPATH_MARKER_ENV);
    } else {
        env.insert("PYTHONSAFEPATH".to_string(), "1".to_string());
        env.insert(HOST_SAFEPATH_MARKER_ENV.to_string(), "1".to_string());
    }
    match env.get("PYTHONPATH").cloned() {
        Some(pythonpath) => {
            env.insert(HOST_PYTHONPATH_ENV.to_string(), pythonpath.clone());
            match strip_cwd_resolving_pythonpath_entries(
                std::ffi::OsStr::new(pythonpath.as_str()),
                cwd,
            ) {
                Some(stripped) => {
                    env.insert(
                        "PYTHONPATH".to_string(),
                        stripped.to_string_lossy().into_owned(),
                    );
                }
                None => {
                    env.remove("PYTHONPATH");
                }
            }
        }
        None => {
            env.insert(
                HOST_PYTHONPATH_ENV.to_string(),
                HOST_PYTHONPATH_UNSET.to_string(),
            );
        }
    }
}

/// Apply the user-project import protection to a command about to run the
/// kernel Python with `-c` in a user project directory: PYTHONSAFEPATH stops
/// the interpreter from prepending the process cwd to `sys.path`, and the
/// inherited PYTHONPATH is stripped of entries that resolve to the child's
/// cwd (which put the project back on `sys.path` at import time and would
/// re-enable that shadowing). Used by the runtime-ready probe and the
/// CLI's factory dispatch — the module's only public item, re-exported at
/// the kernel facade. The child runs no user code of its own, so
/// PYTHONSAFEPATH is set unconditionally, with no marker to pop.
pub fn protect_user_project_imports(command: &mut std::process::Command) {
    command.env("PYTHONSAFEPATH", "1");
    // The entries resolve at import time in the *child's* cwd: the cwd set
    // on this command when there is one (the factory dispatch points at
    // the user's project), else the inherited process cwd.
    let cwd = command
        .get_current_dir()
        .map(std::path::Path::to_path_buf)
        .or_else(|| std::env::current_dir().ok());
    if let Some(pythonpath) = std::env::var_os("PYTHONPATH") {
        match strip_cwd_resolving_pythonpath_entries(&pythonpath, cwd.as_deref()) {
            Some(stripped) => {
                command.env("PYTHONPATH", stripped);
            }
            None => {
                command.env_remove("PYTHONPATH");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a PYTHONPATH value with the platform separator, so the test
    /// is correct on Unix and Windows alike.
    fn pythonpath(entries: &[&str]) -> std::ffi::OsString {
        std::env::join_paths(entries.iter().map(std::path::Path::new))
            .expect("entries without the platform separator")
    }

    /// The stripped PYTHONPATH for these entries, as text (the inputs are
    /// built from str, so no lossy conversion ever fires).
    fn stripped(entries: &[&str], cwd: &std::path::Path) -> Option<String> {
        strip_cwd_resolving_pythonpath_entries(&pythonpath(entries), Some(cwd))
            .map(|value| value.to_string_lossy().into_owned())
    }

    #[test]
    fn strips_entries_that_resolve_to_the_cwd() {
        let root = tempfile::tempdir().expect("tempdir");
        let cwd = root.path();
        std::fs::create_dir(cwd.join("sub")).expect("create sub");
        let cwd_path = cwd.display().to_string();
        // Every one of these resolves to the cwd at import time.
        let cwd_entries = [
            String::new(),
            ".".to_string(),
            "./".to_string(),
            "sub/..".to_string(),
            cwd_path.clone(),
            format!("{cwd_path}{}", std::path::MAIN_SEPARATOR),
        ];
        for entry in &cwd_entries {
            assert_eq!(
                stripped(&[entry, "/x"], cwd),
                Some(pythonpath(&["/x"]).to_string_lossy().into_owned()),
                "a `{entry}` PYTHONPATH entry must be dropped"
            );
        }
    }

    #[test]
    fn keeps_real_entries_and_removes_a_fully_stripped_path() {
        let root = tempfile::tempdir().expect("tempdir");
        let cwd = root.path();
        std::fs::create_dir(cwd.join("sub")).expect("create sub");
        // Real entries stay in their original order; empty ones among them
        // are dropped.
        assert_eq!(
            stripped(&["/a", "", "/b"], cwd),
            Some(pythonpath(&["/a", "/b"]).to_string_lossy().into_owned())
        );
        assert_eq!(
            stripped(&["/a", "/b"], cwd),
            Some(pythonpath(&["/a", "/b"]).to_string_lossy().into_owned())
        );
        // Everything dropped: the variable must be removed entirely.
        assert_eq!(stripped(&["", ""], cwd), None);
        // A project subdirectory entry is out of scope (see the module
        // comment): it stays.
        assert_eq!(
            stripped(&["sub", "/x"], cwd),
            Some(pythonpath(&["sub", "/x"]).to_string_lossy().into_owned())
        );
    }
}
