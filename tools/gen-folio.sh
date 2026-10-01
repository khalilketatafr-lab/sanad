#!/usr/bin/env bash
# Regenerates crates/folio/src/folio_generated.rs from the schema
# with the pinned flatc. The generated file is committed; CI re-runs this and
# fails on any diff.
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
want="$(cat "$root/tools/flatc.version")"
flatc="${FLATC:-flatc}"
have="$("$flatc" --version | awk '{print $3}')"
if [[ "$have" != "$want" ]]; then
  echo "gen-folio: flatc $have found, $want required (must match the flatbuffers crate)" >&2
  exit 1
fi
"$flatc" --rust \
  -o "$root/crates/folio/src/" \
  "$root/crates/folio/schemas/folio.fbs"
echo "gen-folio: regenerated with flatc $have"
