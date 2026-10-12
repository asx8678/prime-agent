# debug: the current working tree, debug build (quick local harness checks).
export PRIME_AGENT_BIN="$(cd "$(dirname "${BASH_SOURCE[0]}")"/../../.. && pwd)/target/debug/prime-agent"
export PRIME_AGENT_FLAGS=()
