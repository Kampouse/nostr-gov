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
  for c in "$HERE/../lisp-rlm" "$HERE/../../lisp-rlm" "$HOME/dev/lisp-rlm" \
           "$HOME/dev/stuff/lisp-rlm" "$HOME/.openclaw/workspace/lisp-rlm"; do
    [ -d "$c" ] && LR="$c" && break
  done
fi

# pick a compiler binary: release → debug → installed (cargo install
# near-compile) → build from source as a last resort
NC=""
if [ -n "$LR" ]; then
  NC="$LR/target/release/near-compile"
  [ -x "$NC" ] || NC="$LR/target/debug/near-compile"
fi
if [ -z "$NC" ] || [ ! -x "$NC" ]; then
  if command -v near-compile >/dev/null 2>&1; then
    NC="$(command -v near-compile)"
  elif [ -n "$LR" ]; then
    echo "→ building near-compile at $LR (cargo build --release; falls back to debug)"
    cargo build --manifest-path "$LR/Cargo.toml" --release --bin near-compile \
      || cargo build --manifest-path "$LR/Cargo.toml" --bin near-compile
    NC="$LR/target/release/near-compile"
    [ -x "$NC" ] || NC="$LR/target/debug/near-compile"
  fi
fi
[ -n "$NC" ] && [ -x "$NC" ] || { echo "✗ near-compile not found (set LISP_RLM_ROOT or cargo install near-compile)"; exit 1; }

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

# sync the web app's bundled binary (public/nostr-gov.wasm) so the UI's
# one-click treasury deployment always ships THIS build — no drift.
# The web e2e gates it: NEAR_GOV_WASM=../public/nostr-gov.wasm python3
# tests/e2e-mock.py must be ALL GREEN before committing.
if [ -d "$HERE/../public" ]; then
  cp "$OUT" "$HERE/../public/nostr-gov.wasm"
  echo "📋 synced → ../public/nostr-gov.wasm ($(wc -c < "$HERE/../public/nostr-gov.wasm" | tr -d ' ') bytes)"
fi
echo "✅ contract ready: $OUT ($(wc -c < "$OUT" | tr -d ' ') bytes)"
