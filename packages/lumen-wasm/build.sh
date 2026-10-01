#!/usr/bin/env bash
# Builds the Compositor for the browser: cargo (wasm32, release) → wasm-bindgen
# (--target web). The wasm-bindgen CLI must match the crate pin exactly.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
want="$(grep -E '^wasm-bindgen = ' "$root/Cargo.toml" | sed -E 's/.*"=([0-9.]+)".*/\1/')"
have="$(wasm-bindgen --version | awk '{print $2}')"
if [[ "$have" != "$want" ]]; then
  echo "lumen-wasm: wasm-bindgen-cli $have found, $want required: cargo install wasm-bindgen-cli --version $want --locked" >&2
  exit 1
fi
cargo build --manifest-path "$root/Cargo.toml" -p sanad-compositor --target wasm32-unknown-unknown --release
rm -rf "$here/pkg"
wasm-bindgen --target web --out-dir "$here/pkg" \
  "$root/target/wasm32-unknown-unknown/release/sanad_compositor.wasm"
gz=$(gzip -9 -c "$here/pkg/sanad_compositor_bg.wasm" | wc -c)
echo "lumen-wasm: sanad_compositor_bg.wasm $(wc -c < "$here/pkg/sanad_compositor_bg.wasm") bytes, ${gz} gz (budget 409600)"
if (( gz > 409600 )); then echo "lumen-wasm: over the 400 KiB gz budget" >&2; exit 1; fi
