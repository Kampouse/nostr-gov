#!/usr/bin/env python3
# Fresh-account sentinel e2e: faucet-create account -> deploy wasm -> full story.
# Usage: python3 tests/e2e-fresh.py [account-id]
import base64, hashlib, json, os, sys, time, urllib.request
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from bip340 import b2i, event_id, i2b, mul, sha, sign
from nacl.signing import SigningKey

RPC = "https://rpc.testnet.fastnear.com"
C = sys.argv[1] if len(sys.argv) > 1 else f"e2esep4.testnet"
GOV = 37500
WASM = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "target", "nostr-gov-ts.wasm")
ALPH = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"
def b58d(s):
    n = 0
    for ch in s: n = n * 58 + ALPH.index(ch)
    raw = n.to_bytes((n.bit_length() + 7) // 8, "big")
    return b"\0" * (len(s) - len(s.lstrip("1"))) + raw
def b58e(b):
    n = int.from_bytes(b, "big"); s = ""
    while n: n, r = divmod(n, 58); s = ALPH[r] + s
    z = len(b) - len(b.lstrip(b"\0"))
    return "1" * z + s

# ── account setup ─────────────────────────────────────────────────────
CREDS = os.path.expanduser(f"~/.near-credentials/testnet/{C}.json")
os.makedirs(os.path.dirname(CREDS), exist_ok=True)
if os.path.exists(CREDS):
    creds = json.load(open(CREDS))
    SEED = b58d(creds["private_key"].split(":", 1)[1])[:32]
else:
    SEED = os.urandom(32)
    creds = None
SK = SigningKey(bytes(SEED)); PUB = bytes(SK.verify_key); PUBSTR = "ed25519:" + b58e(PUB)

def rpc(m, p):
    req = urllib.request.Request(RPC, data=json.dumps({"jsonrpc": "2.0", "id": 1, "method": m, "params": p}).encode(), headers={"Content-Type": "application/json"})
    return json.loads(urllib.request.urlopen(req, timeout=90).read())

def account_exists():
    try:
        r = rpc("query", {"request_type": "view_account", "finality": "final", "account_id": C})
        return isinstance(r.get("result"), dict) and "code_hash" in r["result"]
    except Exception:
        return False

if not account_exists():
    print(f"→ creating {C} via faucet…")
    body = json.dumps({"newAccountId": C, "newAccountPublicKey": PUBSTR}).encode()
    for url in ("https://faucet.testnet.near.org/fund", "https://helper.testnet.near.org/account"):
        try:
            r = urllib.request.urlopen(urllib.request.Request(url, data=body, headers={"Content-Type": "application/json"}), timeout=60)
            print("  faucet:", url, r.read()[:80]); break
        except Exception as e:
            print("  faucet fail:", url, str(e)[:80])
    for _ in range(20):
        if account_exists(): break
        time.sleep(3)
    if not account_exists():
        print("✗ account still missing"); sys.exit(1)
if not creds:
    json.dump({"account_id": C, "public_key": PUBSTR, "private_key": "ed25519:" + b58e(SEED + PUB)}, open(CREDS, "w"))
    os.chmod(CREDS, 0o600)
print(f"✓ account {C} ready")

# ── tx machinery ──────────────────────────────────────────────────────
def bstr(s):
    b = s.encode(); return len(b).to_bytes(4, "little") + b
def bbytes(b):
    return len(b).to_bytes(4, "little") + b

def fn_action(method, args_b, gas=300 * 10**12, dep=0):
    return b"\x02" + bstr(method) + bbytes(args_b) + gas.to_bytes(8, "little") + dep.to_bytes(16, "little")
def deploy_action(wasm_b):
    return b"\x01" + bbytes(wasm_b)

def send(actions, tries=4):
    for attempt in range(tries):
        nonce = rpc("query", {"request_type": "view_access_key", "finality": "final", "account_id": C, "public_key": PUBSTR})["result"]["nonce"]
        bh = b58d(rpc("block", {"finality": "final"})["result"]["header"]["hash"])
        tx = bstr(C) + b"\x00" + PUB + (nonce + 1).to_bytes(8, "little") + bstr(C) + bh + len(actions).to_bytes(4, "little")
        for a in actions: tx += a
        sig = SK.sign(hashlib.sha256(tx).digest()).signature
        r = rpc("broadcast_tx_commit", [base64.b64encode(tx + b"\x00" + sig).decode()])
        if "error" in r:
            s = json.dumps(r["error"])
            if "InvalidNonce" in s and attempt < tries - 1:
                time.sleep(2); continue
            return ("RPC_ERROR", s[:200])
        res = r.get("result", {}); logs = []; fail = None
        for rcpt in res.get("receipts_outcome", []):
            logs += rcpt["outcome"].get("logs", [])
            st = rcpt["outcome"]["status"]
            if "Failure" in st: fail = st["Failure"]; break
        errs = [l for l in logs if "ERR_" in l]
        if fail and not errs: return ("TRAP", json.dumps(fail)[:200])
        return (errs[0].split(":")[0] if errs else "OK", res.get("transaction_hash", "")[:18])
    return ("RPC_ERROR", "nonce retries exhausted")

def call(m, a, dep=0):
    return send([fn_action(m, json.dumps(a).encode(), dep=dep)])
def view(m, a):
    r = rpc("query", {"request_type": "call_function", "account_id": C, "method_name": m,
                      "args_base64": base64.b64encode(json.dumps(a).encode()).decode(), "finality": "final"})
    return bytes(r["result"]["result"]).decode()

# ── deploy wasm (always — idempotent, picks up latest build) ─────────
print("→ deploying wasm…", send([deploy_action(open(WASM, "rb").read())]))

# ── keys & event builders ─────────────────────────────────────────────
TS = int(time.time() * 1e9); EX = TS + 3600 * 10**9
NB = int(time.time()) % 100_000_000  # run-unique nonce base (jump-on-high)
W = f"prod{NB % 1000}"  # unique wallet name
OSK = bytes([0xEE] * 32); OPK = i2b(mul(b2i(OSK))[0]).hex()
A1 = bytes([0xA1] * 32); A1K = i2b(mul(b2i(A1))[0]).hex()
A2 = bytes([0xA2] * 32); A2K = i2b(mul(b2i(A2))[0]).hex()
def sent(s): return s.replace('"', "~")
def ev(action, nonce, sentinel=True):
    tags = f'[["t","nostr-gov"],["action","{action}"],["nonce","{nonce}"],["expires","{EX}"],["contract","{C}"]]'
    ct = "nostr-gov owner action"
    eid = event_id(OPK, 1, GOV, json.loads(tags), ct)
    ser = f'[0,"{OPK}",1,{GOV},{tags},"{ct}"]'
    t, c = (sent(tags), sent(ct)) if sentinel else (tags, ct)
    return {"pk": OPK, "ev": eid, "cat": "1", "kind": str(GOV), "tags": t, "ct": c, "sig": sign(OSK, sha(ser.encode())).hex()}
def appr_event(wallet, pid, ix, sentinel=True):
    msg = f"expires {EX}.000000000: approve:{wallet}:{pid}:{ix} | contract: {C}"
    tags = f'[["contract","{C}"],["wallet","{wallet}"],["proposal","{pid}"],["approver","{ix}"],["action","approve"]]'
    eid = event_id(A2K, 1, GOV, json.loads(tags), msg)
    ser = f'[0,"{A2K}",1,{GOV},{tags},"{msg}"]'
    t, c = (sent(tags), sent(msg)) if sentinel else (tags, msg)
    return {"pk": A2K, "ev": eid, "cat": "1", "kind": str(GOV), "tags": t, "ct": c, "sig": sign(A2, sha(ser.encode())).hex(), "name": wallet, "id": str(pid), "ix": str(ix)}

# ── the story ─────────────────────────────────────────────────────────
steps = []
steps.append(("init", call("init", {"npub": OPK})))
steps.append(("get_version", view("get_version", {})[:40]))
steps.append(("create_wallet", call("create_wallet", dict({"name": W, "pks": f"{A1K},{A2K}", "thr": "2"}, **ev(f"create_wallet:{W}", NB)), dep=10**24)))
steps.append(("wallet_count", view("get_wallet_count", {})[:30]))
steps.append(("propose", call("propose", dict({"name": W, "am": "400000000000000000000000", "rc": C, "pexp": str(EX)}, **ev(f"propose:{W}:{NB+1}", NB+1)))))
pid = str(NB + 1)  # proposal id = the propose event nonce (by contract design)
steps.append(("proposal_ids", view("get_proposal_ids", {"name": W})[:50]))
msg = f"expires {EX}.000000000: approve:{W}:{pid}:0 | contract: {C}"
steps.append(("proposal_msg", view("get_proposal_message", {"name": W, "id": pid, "ix": "0", "exp": str(EX)})[:90]))
steps.append(("approve direct A1", call("approve", {"name": W, "id": pid, "ix": "0", "pubkey_hex": A1K, "signature": sign(A1, sha(msg.encode())).hex(), "expires_at": str(EX)})))
steps.append(("approve_w_ev RAW", call("approve_with_event", appr_event(W, pid, 1, sentinel=False))))
steps.append(("approve_w_ev SENT", call("approve_with_event", appr_event(W, pid, 1, sentinel=True))))
steps.append(("execute", call("execute", dict({"name": W, "id": pid}, **ev(f"execute:{W}:{pid}", NB+2)))))
steps.append(("final proposal", view("get_proposal", {"name": W, "id": pid})[:110]))

print(f"\n{'step':<22} result")
print("─" * 96)
for n, v in steps: print(f"{n:<22} {str(v[0])[:56]:<58} {str(v[1])[:30] if isinstance(v, tuple) and len(v) > 1 else ''}")

# verdict
ok = lambda s: s[1][0] == "OK"
need_ok = ["init", "create_wallet", "propose", "approve direct A1", "approve_w_ev SENT", "execute"]
bad = [n for n in need_ok if not ok(next(s for s in steps if s[0] == n))]
raw = next(s for s in steps if s[0] == "approve_w_ev RAW")[1][0]
print("\nVERDICT:", "ALL GREEN" if not bad else f"FAILED: {bad}", "| RAW step:", raw, "(expected ERR_EVENT_SIG_INVALID)")
