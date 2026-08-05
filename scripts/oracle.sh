#!/usr/bin/env bash
# Diff the Rust port against legacy/sessio.mjs (the original JS implementation, kept in the
# repo purely as this harness's reference) over the same transcripts.
#
# Both implementations dump what load() computed as JSON; this normalises key order and
# compares. Any difference is a port bug until proven otherwise.
#
#   scripts/oracle.sh            # compare, print a summary
#   scripts/oracle.sh -v         # also print the first differing rows
#
# NOT covered by this harness, and needing manual testing:
#   - git-WIP flagging. gitDirty() returns false until a background `git status` resolves, and
#     --dump-json exits before any of them do, so neither side ever reports "uncommitted
#     changes" here.
#   - Anything in the TUI: rendering, layout, keys, resume.

set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
out="${TMPDIR:-/tmp}/sessio-oracle"
mkdir -p "$out"

verbose=0
[[ "${1:-}" == "-v" ]] && verbose=1

command -v jq >/dev/null || { echo "oracle needs jq (brew install jq)" >&2; exit 1; }

bin="$root/target/release/sessio"
[[ -x "$bin" ]] || { echo "build first: cargo build --release" >&2; exit 1; }

echo "→ js"
node "$root/legacy/sessio.mjs" --dump-json | jq -S 'sort_by(.key)' > "$out/js.json"
echo "→ rust"
"$bin" --dump-json | jq -S 'sort_by(.key)' > "$out/rs.json"

js_rows=$(jq 'length' "$out/js.json")
rs_rows=$(jq 'length' "$out/rs.json")
echo
echo "rows:  js=$js_rows  rust=$rs_rows"

if diff -q "$out/js.json" "$out/rs.json" >/dev/null; then
  echo "PARITY: identical across $js_rows rows"
  exit 0
fi

# Per-field breakdown: which fields disagree, and on how many sessions.
jq -n --slurpfile a "$out/js.json" --slurpfile b "$out/rs.json" '
  ($a[0] | map({key: .key, value: .}) | from_entries) as $ja |
  ($b[0] | map({key: .key, value: .}) | from_entries) as $jb |
  ($ja | keys) + ($jb | keys) | unique as $keys |
  [ $keys[] as $k
    | ($ja[$k] // {}) as $x | ($jb[$k] // {}) as $y
    | (($x | keys) + ($y | keys) | unique)[] as $f
    | select(($x[$f] // null) != ($y[$f] // null))
    | $f ]
  | group_by(.) | map({field: .[0], differing_sessions: length})
  | sort_by(-.differing_sessions)
' 2>/dev/null || echo "(field breakdown unavailable)"

if [[ $verbose == 1 ]]; then
  echo
  echo "--- first differing rows ---"
  diff "$out/js.json" "$out/rs.json" | head -40
fi

echo
echo "MISMATCH — artifacts in $out"
exit 1
