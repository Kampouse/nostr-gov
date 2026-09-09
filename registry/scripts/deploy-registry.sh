#!/usr/bin/env bash
# deploy-registry.sh — one-time deployment of the treasury registry.
#
# Creates <account>, deploys public/registry.wasm (build registry/ first).
# Requires the near CLI and a funded testnet account with its access key.
#
#   ./deploy-registry.sh registry.nostrogov.testnet <your-account>.testnet
#
# After deployment every new treasury registers atomically at creation and
# list_by_owner becomes the discovery source (see ../src/lib/constants.ts —
# REGISTRY_CONTRACT must match the account deployed here).
set -eu
REG_ACCT="${1:?usage: deploy-registry.sh <registry-account-id> <deployer-account-id>}"
DEPLOYER="${2:?usage: deploy-registry.sh <registry-account-id> <deployer-account-id>}"
WASM="$(cd "$(dirname "$0")/.." && pwd)/public/registry.wasm"
[ -f "$WASM" ] || { echo "✗ $WASM missing — run registry/build.sh first"; exit 1; }

echo "→ creating ${REG_ACCT} (0.5 Ⓝ for account + storage)"
near create-account "$REG_ACCT" --useAccount "$DEPLOYER" --initialBalance 0.5 || true

echo "→ deploying ${WASM} ($(wc -c < "$WASM" | tr -d ' ') bytes)"
near deploy "$REG_ACCT" "$WASM"

echo "→ verifying"
near view "$REG_ACCT" get_version '{}' || { echo "✗ registry not answering"; exit 1; }
echo "✅ registry live at ${REG_ACCT} — update REGISTRY_CONTRACT in src/lib/constants.ts if it differs"
