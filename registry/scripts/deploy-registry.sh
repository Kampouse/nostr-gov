#!/usr/bin/env bash
# deploy-registry.sh — deploy the treasury registry to NEAR testnet.
#
# PROVEN PATH (2026-09-09, registry-nostrgov.testnet):
#   1. keypair (openssl ed25519) → 2. helper faucet creates+funds the
#      top-level account → 3. near-compile deploy (signs from
#      ~/.near-credentials) → 4. register/unregister via near-compile call.
# The near-cli-rs alternative demands an interactive confirmation that
# scripted environments can't provide — hence this script's openssl+faucet
# path for account creation.
#
# Usage: ./deploy-registry.sh <account-id>   (default registry-nostrgov.testnet)
set -eu
ACCT="${1:-registry-nostrgov.testnet}"
HERE="$(cd "$(dirname "$0")/.." && pwd)"
NC="${NEAR_COMPILE:-$(command -v near-compile || echo "$HERE/../../lisp-rlm/target/debug/near-compile")}"
WASM="$HERE/../public/registry.wasm"
CRED="$HOME/.near-credentials/testnet/$ACCT.json"
[ -f "$WASM" ] || { echo "✗ $WASM missing — run ../build.sh first"; exit 1; }

if [ ! -f "$CRED" ]; then
  echo "→ generating keypair + creating $ACCT via testnet helper faucet"
  python3 - "$ACCT" <<'PYEOF'
import base64, json, os, subprocess, sys, urllib.request, tempfile
acct = sys.argv[1]
key = subprocess.run(["openssl","genpkey","-algorithm","ed25519"],
                     capture_output=True, text=True, check=True).stdout
with tempfile.NamedTemporaryFile("w", suffix=".pem", delete=False) as f:
    f.write(key); kp = f.name
der = subprocess.run(["openssl","pkey","-in",kp,"-outform","DER"],
                     capture_output=True, check=True).stdout
seed = der[-32:]
pub = subprocess.run(["openssl","pkey","-in",kp,"-pubout","-outform","DER"],
                     capture_output=True, check=True).stdout
pub32 = pub[-32:]
alpha = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"
def b58(b):
    n = int.from_bytes(b,"big"); s = ""
    while n: n,r = divmod(n,58); s = alpha[r]+s
    for byte in b:
        if byte == 0: s = "1"+s
        else: break
    return s or "1"
pk58 = b58(pub32)
req = urllib.request.Request("https://helper.testnet.near.org/account",
    data=json.dumps({"newAccountId": acct,
                     "newAccountPublicKey": f"ed25519:{pk58}"}).encode(),
    headers={"Content-Type":"application/json"})
with urllib.request.urlopen(req, timeout=60) as r:
    print("  faucet:", r.status)
cred = {"account_id": acct, "public_key": f"ed25519:{pk58}",
        "private_key": f"ed25519:{b58(seed+pub32)}"}
os.makedirs(os.path.expanduser("~/.near-credentials/testnet"), exist_ok=True)
json.dump(cred, open(os.path.expanduser(
    f"~/.near-credentials/testnet/{acct}.json"),"w"), indent=2)
os.unlink(kp)
print("  credentials saved")
PYEOF
fi

echo "→ deploying $WASM ($(wc -c < "$WASM" | tr -d ' ') bytes) via near-compile"
( cd "$HERE" && "$NC" deploy --account "$ACCT" )

echo "→ verifying"
"$NC" view "$ACCT" get_version '{}' --account "$ACCT" 2>/dev/null \
  || curl -s https://rpc.testnet.near.org -H 'Content-Type: application/json' \
    -d "{\"jsonrpc\":\"2.0\",\"id\":\"1\",\"method\":\"query\",\"params\":{\"request_type\":\"call_function\",\"finality\":\"final\",\"account_id\":\"$ACCT\",\"method_name\":\"get_version\",\"args_base64\":\"e30=\"}}" \
    | python3 -c "import json,sys; print('get_version:', bytes(json.load(sys.stdin)['result']['result']).decode())"
echo "✅ registry live at $ACCT — REGISTRY_CONTRACT in src/lib/constants.ts must match"
