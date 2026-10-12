# baseline: the current working tree, release build, default flags.
# A before/after run pairs this with another config (e.g. configs/main.sh
# pointing at a build of the main branch) and compares them with analyze.py.
export PRIME_AGENT_BIN="$(cd "$(dirname "${BASH_SOURCE[0]}")"/../../.. && pwd)/target/release/prime-agent"
export PRIME_AGENT_FLAGS=()
