#!/usr/bin/env bash
# Generate only the README screenshots. Website assets remain unchanged.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$ROOT/assets/readme"
TMP="$(mktemp -d "${TMPDIR:-/tmp}/agentstmux-images.XXXXXX")"
cleanup() {
    if [[ "$TMP" == */agentstmux-images.* ]]; then rm -rf "$TMP"; fi
}
trap cleanup EXIT

cargo build --release --manifest-path "$ROOT/Cargo.toml"
bash "$ROOT/fixtures/scenarios/readme/scenario.sh" "$TMP"
(
    cd "$ROOT/website"
    node ../scripts/render-frames.mjs "$TMP" --scale 2 --readme
)
mkdir -p "$OUT"
cp "$TMP/overview.png" "$OUT/overview.png"
cp "$TMP/codex-usage.png" "$OUT/codex-usage.png"
echo "README screenshots saved to $OUT"
