#!/usr/bin/env bash
# Build the browser demo the site runs.
#
# The demo is the real dashboard compiled to wasm, not an imitation of it: the page calls the same
# `frame_lines` the terminal does. That is the whole point — the previous site hand-wrote its
# terminal mock in HTML and it drifted until it documented keys that no longer existed.
#
# The Pages deploy (.github/workflows/pages.yml) runs this on every push to main, so the site always
# shows the layout it was deployed with, and CI runs it on every PR. docs/demo/ is not committed.
# Run it by hand only to preview the site locally.
set -euo pipefail
cd "$(dirname "$0")/.."

OUT=docs/demo
BINDGEN_VERSION=$(grep -A1 'name = "wasm-bindgen"' Cargo.lock | grep version | head -1 | sed 's/.*"\(.*\)".*/\1/')

command -v wasm-bindgen >/dev/null || {
  echo "wasm-bindgen not found. Install the version matching the crate:" >&2
  echo "  cargo install wasm-bindgen-cli --version $BINDGEN_VERSION" >&2
  exit 1
}

have=$(wasm-bindgen --version | awk '{print $2}')
[ "$have" = "$BINDGEN_VERSION" ] || {
  echo "wasm-bindgen CLI is $have but the crate is $BINDGEN_VERSION — they must match." >&2
  echo "  cargo install wasm-bindgen-cli --version $BINDGEN_VERSION" >&2
  exit 1
}

echo "building wasm…"
cargo build --release --lib --target wasm32-unknown-unknown

echo "generating bindings…"
rm -rf "$OUT"
wasm-bindgen --target web --no-typescript --out-dir "$OUT" \
  target/wasm32-unknown-unknown/release/sessio.wasm

# wasm-opt is worth ~40% but is not worth failing the build over.
if command -v wasm-opt >/dev/null; then
  echo "optimising…"
  # wasm-bindgen drops the section that tells wasm-opt which features rustc used, so they are named
  # here: rustc's default wasm32 set. Without them wasm-opt rejects the module (memory.copy needs
  # bulk memory).
  wasm-opt -Oz \
    --enable-bulk-memory --enable-nontrapping-float-to-int --enable-sign-ext \
    --enable-mutable-globals --enable-reference-types --enable-multivalue \
    -o "$OUT/sessio_bg.wasm" "$OUT/sessio_bg.wasm"
fi

echo "done → $OUT ($(du -h "$OUT/sessio_bg.wasm" | cut -f1))"
