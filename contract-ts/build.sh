#!/usr/bin/env bash
# build.sh — compile the lisp-rlm-dialect TypeScript contract to NEAR wasm.
#
# Requires the lisp-rlm toolchain (github.com/Kampouse/lisp-rlm):
#   LISP_RLM_ROOT=<path>   (default: ../lisp-rlm or ~/dev/lisp-rlm)
# Pipeline: ts_frontend lowers TS → lisp source, then the standard NEAR
# pipeline parses, type-checks and emits wasm.
set -eu
HERE="$(cd "$(dirname "$0")" && pwd)"
OUT="${1:-$HERE/target/nostr-gov-ts.wasm}"
SRC="$HERE/src/main.ts"

# resolve lisp-rlm root
LR="${LISP_RLM_ROOT:-}"
if [ -z "$LR" ]; then
  for c in "$HERE/../lisp-rlm" "$HOME/dev/lisp-rlm" "$HOME/.openclaw/workspace/lisp-rlm"; do
    [ -d "$c" ] && LR="$c" && break
  done
fi
[ -n "$LR" ] && [ -x "$LR/target/release/compile" ] || {
  echo "→ building lisp-rlm compiler at $LR (first run: cargo build --release --bin compile)"
  cargo build --manifest-path "$LR/Cargo.toml" --release --bin compile
}
[ -x "$LR/target/release/compile" ] || { echo "✗ lisp-rlm compiler not found (set LISP_RLM_ROOT)"; exit 1; }

mkdir -p "$(dirname "$OUT")"
rm -f "$OUT"
"$LR/target/release/compile" "$SRC" "$OUT" 2>&1 | grep -vE '^(START|Reading|Parsed)' || true
[ -f "$OUT" ] || { echo "✗ compile failed — $OUT not produced"; exit 1; }

# optional wasm-opt shrink (-g keeps the name section for trap symbolication)
if command -v wasm-opt >/dev/null 2>&1; then
  wasm-opt --enable-bulk-memory-opt -g -Oz "$OUT" -o "$OUT.opt" \
    && wasm-tools validate "$OUT.opt" 2>/dev/null || true
  if [ -f "$OUT.opt" ]; then mv "$OUT.opt" "$OUT"; fi
fi
echo "✅ contract ready: $OUT ($(wc -c < "$OUT" | tr -d ' ') bytes)"
