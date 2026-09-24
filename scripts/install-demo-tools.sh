#!/usr/bin/env bash
# Install what scripts/build-demo.sh needs on a GitHub Actions Linux x86_64 runner: the wasm32
# target, the wasm-bindgen CLI at exactly the version Cargo.lock pins, and binaryen's wasm-opt.
#
# Both come as prebuilt release tarballs, checked against their published sha256, so this costs
# seconds rather than the minutes `cargo install` would. Used by ci.yml and pages.yml; locally,
# follow the README instead.
set -euo pipefail
cd "$(dirname "$0")/.."

# Bump deliberately: a new wasm-opt can change the output, or refuse what rustc emits.
BINARYEN_VERSION=version_133

BINDGEN_VERSION=$(grep -A1 'name = "wasm-bindgen"' Cargo.lock | grep version | head -1 | sed 's/.*"\(.*\)".*/\1/')
[ -n "$BINDGEN_VERSION" ] || { echo "wasm-bindgen not found in Cargo.lock" >&2; exit 1; }

DEST="${RUNNER_TEMP:-$(mktemp -d)}/demo-tools"
mkdir -p "$DEST"
cd "$DEST"

fetch() { # url of a tarball whose checksum sits beside it with the given suffix
  local url=$1 sum=$2 file
  file=$(basename "$url")
  curl -fsSLO "$url"
  curl -fsSL "$url$sum" -o "$file.sum"
  # The checksum files name the tarball; compare the hash alone so a path in them cannot matter.
  echo "$(awk '{print $1}' "$file.sum")  $file" | sha256sum -c -
  tar -xzf "$file"
}

rustup target add wasm32-unknown-unknown

fetch "https://github.com/wasm-bindgen/wasm-bindgen/releases/download/$BINDGEN_VERSION/wasm-bindgen-$BINDGEN_VERSION-x86_64-unknown-linux-musl.tar.gz" .sha256sum
fetch "https://github.com/WebAssembly/binaryen/releases/download/$BINARYEN_VERSION/binaryen-$BINARYEN_VERSION-x86_64-linux.tar.gz" .sha256

BIN_DIRS=("$DEST/wasm-bindgen-$BINDGEN_VERSION-x86_64-unknown-linux-musl" "$DEST/binaryen-$BINARYEN_VERSION/bin")
for d in "${BIN_DIRS[@]}"; do
  if [ -n "${GITHUB_PATH:-}" ]; then echo "$d" >> "$GITHUB_PATH"; else echo "add to PATH: $d"; fi
done

"${BIN_DIRS[0]}/wasm-bindgen" --version
"${BIN_DIRS[1]}/wasm-opt" --version
