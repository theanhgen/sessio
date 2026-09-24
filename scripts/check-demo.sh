#!/usr/bin/env bash
# Check the website demo scripts/build-demo.sh just built, as CI does after building it.
#
# The demo is built from the commit being deployed (pages.yml) and on every pull request
# (ci.yml), so the wasm the site runs always comes from the renderer in the same commit. What
# that cannot catch on its own:
#
# - a docs/demo/ committed by hand, which is gitignored for exactly that reason (#15): the site
#   would serve it alongside a build it no longer matches;
# - a build that "succeeds" without its output, or with a module the page cannot use: every
#   function docs/index.html calls on the module must be one the bindings export.
set -euo pipefail
cd "$(dirname "$0")/.."

fail() { echo "check-demo: $*" >&2; exit 1; }

tracked=$(git ls-files docs/demo)
[ -z "$tracked" ] || fail "docs/demo/ is built on deploy and must not be committed:
$tracked"

for f in docs/demo/sessio.js docs/demo/sessio_bg.wasm; do
  [ -s "$f" ] || fail "$f is missing or empty — run scripts/build-demo.sh first"
done

grep -q "import('./demo/sessio.js')" docs/index.html || fail "docs/index.html no longer imports ./demo/sessio.js"

for fn in $(grep -o 'wasm\.[a-z_]*' docs/index.html | sed 's/wasm\.//' | sort -u); do
  if [ "$fn" = default ]; then
    grep -Eq '^export default|as default }' docs/demo/sessio.js || fail "sessio.js has no default export (the init)"
  else
    grep -Eq "^export function $fn\(" docs/demo/sessio.js || fail "docs/index.html calls wasm.$fn, which sessio.js does not export"
  fi
done

echo "check-demo: ok ($(du -h docs/demo/sessio_bg.wasm | cut -f1) wasm; page calls $(grep -o 'wasm\.[a-z_]*' docs/index.html | sort -u | tr '\n' ' '))"
