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
[ -n "$LR" ] && NC="$LR/target/release/near-compile"
[ -n "$LR" ] && [ -x "$NC" ] || NC="$LR/target/debug/near-compile"
if [ -z "$NC" ] || [ ! -x "$NC" ]; then
  echo "→ building lisp-rlm compiler at $LR (cargo build --release --bin near-compile; falls back to debug)"
  cargo build --manifest-path "$LR/Cargo.toml" --release --bin near-compile \
    || cargo build --manifest-path "$LR/Cargo.toml" --bin near-compile
  NC="$LR/target/release/near-compile"
  [ -x "$NC" ] || NC="$LR/target/debug/near-compile"
fi
[ -n "$NC" ] && [ -x "$NC" ] || { echo "✗ near-compile not found (set LISP_RLM_ROOT)"; exit 1; }
# alt: the standalone crate — cargo install near-compile (v0.1.1+ has the TS dispatch)
if ! [ -x "$NC" ]; then
  command -v near-compile >/dev/null 2>&1 && NC="$(command -v near-compile)"
fi
[ -x "$NC" ] || { echo "✗ near-compile binary not found (set LISP_RLM_ROOT or cargo install near-compile)"; exit 1; }

mkdir -p "$(dirname "$OUT")"
rm -f "$OUT"
"$NC" "$SRC" "$OUT" 2>&1 | grep -vE '^(START|Reading|Parsed)' || true
[ -f "$OUT" ] || { echo "✗ compile failed — $OUT not produced"; exit 1; }

# optional wasm-opt shrink (-g keeps the name section for trap symbolication)
if command -v wasm-opt >/dev/null 2>&1; then
  wasm-opt --enable-bulk-memory-opt -g -Oz "$OUT" -o "$OUT.opt" \
    && wasm-tools validate "$OUT.opt" 2>/dev/null || true
  if [ -f "$OUT.opt" ]; then mv "$OUT.opt" "$OUT"; fi
fi
echo "✅ contract ready: $OUT ($(wc -c < "$OUT" | tr -d ' ') bytes)"
