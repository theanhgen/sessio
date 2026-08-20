#!/usr/bin/env bash
# Build the per-platform npm packages that carry the sessio binaries.
#
# The root `sessio` package lists these as optionalDependencies, so npm downloads exactly the
# one matching the installing machine. Each package declares `os`/`cpu` so npm skips the rest.
#
#   scripts/stage-npm-packages.sh <version> <artifact-dir> <out-dir>
#
# <artifact-dir> holds one subdirectory per rust target triple, each containing `sessio`.
# Produces <out-dir>/<pkg-name>/ ready for `npm publish`.

set -euo pipefail

version="${1:?usage: stage-npm-packages.sh <version> <artifact-dir> <out-dir>}"
artifacts="${2:?missing artifact dir}"
out="${3:?missing out dir}"

# rust target triple -> npm package suffix, npm os, npm cpu, libc
targets=(
  "aarch64-apple-darwin:darwin-arm64:darwin:arm64:"
  "x86_64-apple-darwin:darwin-x64:darwin:x64:"
  "aarch64-unknown-linux-gnu:linux-arm64:linux:arm64:glibc"
  "x86_64-unknown-linux-gnu:linux-x64:linux:x64:glibc"
  "x86_64-unknown-linux-musl:linux-x64-musl:linux:x64:musl"
)

mkdir -p "$out"
staged=0

for entry in "${targets[@]}"; do
  IFS=: read -r triple suffix os cpu libc <<<"$entry"
  src="$artifacts/$triple/sessio"
  if [[ ! -f "$src" ]]; then
    echo "skip $triple (no artifact at $src)" >&2
    continue
  fi

  pkg="$out/sessio-$suffix"
  mkdir -p "$pkg/bin"
  install -m 0755 "$src" "$pkg/bin/sessio"

  libc_field=""
  [[ -n "$libc" ]] && libc_field=$'\n  "libc": ["'"$libc"$'"],'

  cat > "$pkg/package.json" <<JSON
{
  "name": "sessio-$suffix",
  "version": "$version",
  "description": "sessio native binary for $os-$cpu${libc:+ ($libc)}",
  "os": ["$os"],
  "cpu": ["$cpu"],$libc_field
  "files": ["bin/"],
  "license": "MIT",
  "repository": {
    "type": "git",
    "url": "git+https://github.com/theanhgen/sessio.git"
  }
}
JSON

  cat > "$pkg/README.md" <<MD
# sessio-$suffix

Native binary for [sessio](https://www.npmjs.com/package/sessio) on $os-$cpu${libc:+ ($libc)}.

Do not install this directly — install \`sessio\`, which pulls in the right binary for your
machine as an optional dependency.
MD

  echo "staged sessio-$suffix"
  staged=$((staged + 1))
done

if [[ $staged -eq 0 ]]; then
  echo "error: no artifacts found under $artifacts" >&2
  exit 1
fi
echo "staged $staged package(s) in $out"
