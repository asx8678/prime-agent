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
//! A user's set-but-empty `PYTHONSAFEPATH=` reaches `bash()` children as
//! unset instead of being restored verbatim: Python treats the two
//! identically (`sys.flags.safe_path` is False either way), so a second
//! record-and-restore pair would add wire surface for no observable
//! difference.
//!
//! The module is crate-internal except for
//! [`protect_user_project_imports`], which pa-cli's factory dispatch
//! needs (re-exported at the kernel facade).

/// Marker the host sets next to the PYTHONSAFEPATH it injects. The runtime
/// pops the pair at the very start of `rlm.repl.main()` before any cell or
/// `bash()` child can inherit them, so a PYTHONSAFEPATH the user set
/// themselves (no marker) survives.
const HOST_SAFEPATH_MARKER_ENV: &str = "PRIME_AGENT_KERNEL_SAFEPATH";

/// The original PYTHONPATH the host records next to the entries it strips
/// from the kernel's environment. The runtime pops it at the very start of
/// `rlm.repl.main()` and restores (or, for the sentinel, removes) PYTHONPATH
/// before any cell or `bash()` child can inherit the stripped value: the
/// strip protects the runtime's own imports, not the agent's commands.
/// Always written when the host applies its protection, so a stale inherited
/// value can never survive into a fresh kernel.
const HOST_PYTHONPATH_ENV: &str = "PRIME_AGENT_KERNEL_PYTHONPATH";

/// Sentinel recorded in [`HOST_PYTHONPATH_ENV`] when PYTHONPATH was unset:
/// a leading `\u{1}` cannot occur in a real path list (Windows forbids
/// control characters in filenames; on every platform it is not a path
/// anyone sets). Must stay byte-identical to `_HOST_PYTHONPATH_UNSET` in
/// `prime-agent-runtime/src/rlm/repl.py`.
const HOST_PYTHONPATH_UNSET: &str = "\u{1}";

/// The delimiter `CPython` splits PYTHONPATH on: the platform separator,
/// byte against byte, with no quoting convention. Rust's
/// `std::env::split_paths`/`join_paths` follow PATH's double-quote rules
/// instead — on Windows `C:\a";.;"C:\b` is one entry to them but three to
/// Python, including `.`, which would bypass the strip — so the raw
/// delimiter is the only faithful mirror of what the interpreter does
/// with the value.
const PYTHONPATH_DELIMITER: &str = if cfg!(windows) { ";" } else { ":" };

/// Split a PYTHONPATH value the way `CPython` does: on the raw platform
/// delimiter (see [`PYTHONPATH_DELIMITER`]), no quote handling.
fn split_pythonpath(value: &std::ffi::OsStr) -> Vec<std::ffi::OsString> {
    // The delimiter is one ASCII byte, so it can never appear inside a
    // multi-byte sequence of the platform encoding: every split below
    // lands on an encoding boundary.
    let bytes = value.as_encoded_bytes();
    let delimiter = PYTHONPATH_DELIMITER.as_bytes()[0];
    let mut entries = Vec::new();
    let mut start = 0;
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == delimiter {
            // SAFETY: the subslice starts at `bytes`' own start or right
            // after a delimiter byte, and ends on one, so it is a whole
            // number of encoded code points.
            entries.push(
                unsafe { std::ffi::OsStr::from_encoded_bytes_unchecked(&bytes[start..index]) }
                    .to_owned(),
            );
            start = index + 1;
        }
    }
    // SAFETY: the final entry starts on the same kind of boundary.
    entries
        .push(unsafe { std::ffi::OsStr::from_encoded_bytes_unchecked(&bytes[start..]) }.to_owned());
    entries
}

/// Join PYTHONPATH entries back on the raw platform delimiter. Unlike
/// `std::env::join_paths` this cannot fail — any byte sequence is
/// expressible — but an entry that itself contains the delimiter is
/// indistinguishable from two entries, exactly the ambiguity Python has.
fn join_pythonpath(entries: &[std::ffi::OsString]) -> std::ffi::OsString {
    let mut joined = std::ffi::OsString::new();
    for (index, entry) in entries.iter().enumerate() {
        if index > 0 {
            joined.push(std::ffi::OsStr::new(PYTHONPATH_DELIMITER));
        }
        joined.push(entry);
    }
    joined
}

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
fn strip_cwd_resolving_pythonpath_entries(
    value: &std::ffi::OsStr,
    cwd: Option<&std::path::Path>,
) -> Option<std::ffi::OsString> {
    // The identities the cwd can compare under, computed once for every
    // entry checked below.
    let cwd_identities: Vec<std::path::PathBuf> = cwd.map(path_identities).unwrap_or_default();
    let kept: Vec<std::ffi::OsString> = split_pythonpath(value)
        .into_iter()
        .filter(|entry| {
            // An empty entry resolves to the child's cwd at import time.
            if entry.is_empty() {
                return false;
            }
            let Some(cwd) = cwd else {
                // No cwd to judge against: keep the entry.
                return true;
            };
            // An entry resolves to `cwd` literally, through `.`, `./` or
            // `sub/..`, or as the absolute path. Existing paths compare
            // canonically (symlinks included); the lexical identity in
            // [`path_identities`] covers paths that do not exist yet,
            // where the OS cannot canonicalize.
            let entry_path = std::path::Path::new(entry);
            let joined = if entry_path.is_absolute() {
                entry_path.to_path_buf()
            } else {
                cwd.join(entry_path)
            };
            !path_identities(&joined)
                .iter()
                .any(|identity| cwd_identities.contains(identity))
        })
        .collect();
    if kept.is_empty() {
        // Every entry dropped: the variable must be removed entirely.
        // Python 3.11+ treats an empty PYTHONPATH exactly like an unset
        // one (it is `PYTHONPATH=:` that puts the cwd back on sys.path),
        // and an older interpreter — one that ignores PYTHONSAFEPATH —
        // would read an empty entry as the cwd, so removal is the safe
        // form everywhere.
        None
    } else {
        Some(join_pythonpath(&kept))
    }
}

/// The identities a path can compare under: the canonical form when the
/// path exists, and the absolute form with `.`, `..` and duplicate
/// separators collapsed purely lexically — no filesystem access, no
/// symlink resolution (`std::path::absolute` already normalizes on
/// Windows; on Unix it does not, so the component walk does it) — which
/// covers paths that do not exist yet, where the OS cannot canonicalize.
fn path_identities(path: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut identities = Vec::new();
    if let Ok(canon) = std::fs::canonicalize(path) {
        identities.push(canon);
    }
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut normalized = std::path::PathBuf::new();
    for component in absolute.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            std::path::Component::RootDir
            | std::path::Component::Prefix(_)
            | std::path::Component::Normal(_) => normalized.push(component.as_os_str()),
        }
    }
    identities.push(normalized);
    identities
}

/// Environment lookup for the variable names `CPython` itself reads: exact
/// on Unix, case-insensitive on Windows, where `PythonPath` is the same
/// variable as `PYTHONPATH`.
fn env_lookup(env: &std::collections::HashMap<String, String>, name: &str) -> Option<String> {
    if cfg!(windows) {
        env.iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.clone())
    } else {
        env.get(name).cloned()
    }
}

/// Remove `name` under every casing Windows considers its equal (on
/// case-sensitive platforms a plain exact remove).
fn env_remove(env: &mut std::collections::HashMap<String, String>, name: &str) {
    if cfg!(windows) {
        env.retain(|key, _| !key.eq_ignore_ascii_case(name));
    } else {
        env.remove(name);
    }
}

/// Set `name` to `value`, replacing whatever casing it was set under.
fn env_set(env: &mut std::collections::HashMap<String, String>, name: &str, value: &str) {
    env_remove(env, name);
    env.insert(name.to_string(), value.to_string());
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
    if env_lookup(env, "PYTHONSAFEPATH").is_some_and(|value| !value.is_empty()) {
        env_remove(env, HOST_SAFEPATH_MARKER_ENV);
    } else {
        env_set(env, "PYTHONSAFEPATH", "1");
        env_set(env, HOST_SAFEPATH_MARKER_ENV, "1");
    }
    match env_lookup(env, "PYTHONPATH") {
        Some(pythonpath) => {
            env_set(env, HOST_PYTHONPATH_ENV, &pythonpath);
            match strip_cwd_resolving_pythonpath_entries(std::ffi::OsStr::new(&pythonpath), cwd) {
                // The map's values are String, so the lossy conversion
                // never fires.
                Some(stripped) => env_set(env, "PYTHONPATH", &stripped.to_string_lossy()),
                None => env_remove(env, "PYTHONPATH"),
            }
        }
        None => env_set(env, HOST_PYTHONPATH_ENV, HOST_PYTHONPATH_UNSET),
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

    /// Build a PYTHONPATH value with the raw platform delimiter, the way
    /// Python reads one back.
    fn pythonpath(entries: &[&str]) -> std::ffi::OsString {
        join_pythonpath(
            &entries
                .iter()
                .map(|entry| std::ffi::OsString::from(*entry))
                .collect::<Vec<_>>(),
        )
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

    /// The split/join pair mirrors Python exactly: the raw platform
    /// delimiter, nothing else, so a value rebuilt from entries reads
    /// back as those entries.
    #[test]
    fn split_and_join_round_trip_on_the_raw_platform_delimiter() {
        let entries = ["", ".", "/x", ""];
        let joined = pythonpath(&entries);
        assert_eq!(
            split_pythonpath(&joined),
            entries
                .iter()
                .map(|entry| std::ffi::OsString::from(*entry))
                .collect::<Vec<_>>()
        );
    }

    /// `CPython` applies no quote handling: `C:\a";.;"C:\b` is three entries
    /// to Python, the middle one the cwd. `std::env::split_paths` follows
    /// PATH's double-quote convention and would see one entry, keeping the
    /// cwd entry and bypassing the strip.
    #[test]
    #[cfg(windows)]
    fn quoted_cwd_entries_split_like_python_splits_them() {
        let project = tempfile::tempdir().expect("tempdir");
        let cwd = project.path();
        let value = std::ffi::OsString::from(r#"C:\a";.;"C:\b"#);
        assert_eq!(
            split_pythonpath(&value),
            [
                std::ffi::OsString::from(r#"C:\a""#),
                std::ffi::OsString::from("."),
                std::ffi::OsString::from(r#""C:\b"#),
            ]
        );
        assert_eq!(
            strip_cwd_resolving_pythonpath_entries(&value, Some(cwd)),
            Some(pythonpath(&[r#"C:\a""#, r#""C:\b"#])),
            "the quoted `.` entry is the cwd and must be dropped"
        );
    }

    /// Windows environment names are case-insensitive: a mixed-case
    /// `PythonPath` is the same variable and must be handled where it
    /// stands, not duplicated under the canonical spelling.
    #[test]
    #[cfg(windows)]
    fn mixed_case_env_names_are_handled() {
        let project = tempfile::tempdir().expect("tempdir");
        let mut env = std::collections::HashMap::new();
        env.insert("PythonSafePath".to_string(), "1".to_string());
        env.insert("PythonPath".to_string(), ".".to_string());
        protect_kernel_env(&mut env, Some(project.path()));
        // The user's safepath stayed, marker-free.
        assert_eq!(env_lookup(&env, "PYTHONSAFEPATH").as_deref(), Some("1"));
        assert!(env
            .iter()
            .all(|(key, _)| !key.eq_ignore_ascii_case(HOST_SAFEPATH_MARKER_ENV)));
        // The mixed-case PYTHONPATH was recorded, fully stripped (a lone
        // `.`), and removed under its own casing.
        assert_eq!(
            env.get(HOST_PYTHONPATH_ENV).map(String::as_str),
            Some("."),
            "the original is recorded under the host's own name"
        );
        assert!(
            env.iter()
                .all(|(key, _)| !key.eq_ignore_ascii_case("PYTHONPATH")),
            "the mixed-case PYTHONPATH was removed, not duplicated"
        );
    }
}
