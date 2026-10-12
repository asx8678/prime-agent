# debug-paired: the same debug build under a second config name, for
# verifying the paired-analysis path of a run (before/after with identical
# binaries should report zero deltas).
export PRIME_AGENT_BIN="$(cd "$(dirname "${BASH_SOURCE[0]}")"/../../.. && pwd)/target/debug/prime-agent"
export PRIME_AGENT_FLAGS=()
