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
