#!/usr/bin/env python3
# Watcher E2E: fresh wallet+proposal -> sign kind-37500 approval ->
# POST to live CF worker /ingest -> watcher submits on-chain (its gas) ->
# poll contract until approval lands -> execute.
import base64, hashlib, json, os, sys, time, urllib.request
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "tests"))
from bip340 import b2i, event_id, i2b, mul, sha, sign
from nacl.signing import SigningKey

RPC = "https://rpc.testnet.fastnear.com"
C = "nostrgove2e.testnet"
WORKER = "https://nostr-relay-watcher.kj95hgdgnn.workers.dev"
GOV = 37500
ALPH = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"
def b58d(s):
    n = 0
    for ch in s: n = n * 58 + ALPH.index(ch)
    raw = n.to_bytes((n.bit_length() + 7) // 8, "big")
    return b"\0" * (len(s) - len(s.lstrip("1"))) + raw

creds = json.load(open(os.path.expanduser(f"~/.near-credentials/testnet/{C}.json")))
SEED = b58d(creds["private_key"].split(":", 1)[1])[:32]
PUB = b58d(creds["public_key"].split(":", 1)[1])
SK = SigningKey(SEED); PUBSTR = creds["public_key"]
assert bytes(SK.verify_key) == PUB

def bstr(s):
    b = s.encode(); return len(b).to_bytes(4, "little") + b
def bbytes(b):
    return len(b).to_bytes(4, "little") + b
_id = [0]
def rpc(m, p):
    import time as _t
    for _ in range(4):
        _id[0] += 1
        try:
            req = urllib.request.Request(RPC, data=json.dumps({"jsonrpc": "2.0", "id": _id[0], "method": m, "params": p}).encode(), headers={"Content-Type": "application/json"})
            return json.loads(urllib.request.urlopen(req, timeout=90).read())
        except Exception as e:
            if _ == 3: raise
            _t.sleep(3)
def call(m, a, dep=0):
    for _ in range(4):
        n = rpc("query", {"request_type": "view_access_key", "finality": "final", "account_id": C, "public_key": PUBSTR})["result"]["nonce"]
        bh = b58d(rpc("block", {"finality": "final"})["result"]["header"]["hash"])
        ab = json.dumps(a).encode()
        act = b"\x02" + bstr(m) + bbytes(ab) + (300 * 10**12).to_bytes(8, "little") + dep.to_bytes(16, "little")
        tx = bstr(C) + b"\x00" + PUB + (n + 1).to_bytes(8, "little") + bstr(C) + bh + (1).to_bytes(4, "little") + act
        sig = SK.sign(hashlib.sha256(tx).digest()).signature
        r = rpc("broadcast_tx_commit", [base64.b64encode(tx + b"\x00" + sig).decode()])
        if "error" in r:
            if "InvalidNonce" in json.dumps(r["error"]):
                time.sleep(2); continue
            return ("RPC_ERROR", json.dumps(r["error"])[:160])
        res = r.get("result", {}); logs = []; fail = None
        for rcpt in res.get("receipts_outcome", []):
            logs += rcpt["outcome"].get("logs", [])
            st = rcpt["outcome"]["status"]
            if "Failure" in st: fail = st["Failure"]; break
        errs = [l for l in logs if "ERR_" in l]
        if fail and not errs: return ("TRAP", json.dumps(fail)[:160])
        return (errs[0].split(":")[0] if errs else "OK", res.get("transaction_hash", "")[:18])
    return ("RPC_ERROR", "nonce exhausted")
def view(m, a):
    r = rpc("query", {"request_type": "call_function", "account_id": C, "method_name": m,
                      "args_base64": base64.b64encode(json.dumps(a).encode()).decode(), "finality": "final"})
    return bytes(r["result"]["result"]).decode()

TS = int(time.time() * 1e9); EX = TS + 3600 * 10**9
NB = int(time.time()) % 100_000_000 + 500_000
W = f"wdr{NB % 10000}"; PID = NB + 1
OSK = bytes([0xEE] * 32); OPK = i2b(mul(b2i(OSK))[0]).hex()
A1 = bytes([0xA1] * 32); A1K = i2b(mul(b2i(A1))[0]).hex()
A2 = bytes([0xA2] * 32); A2K = i2b(mul(b2i(A2))[0]).hex()
def ev(action, nonce):
    tags = f'[["t","nostr-gov"],["action","{action}"],["nonce","{nonce}"],["expires","{EX}"],["contract","{C}"]]'
    ct = "nostr-gov owner action"
    eid = event_id(OPK, 1, GOV, json.loads(tags), ct)
    ser = f'[0,"{OPK}",1,{GOV},{tags},"{ct}"]'
    return {"pk": OPK, "ev": eid, "cat": "1", "kind": str(GOV), "tags": tags.replace('"', "~"), "ct": ct, "sig": sign(OSK, sha(ser.encode())).hex()}

r1 = call("create_wallet", dict({"name": W, "pks": f"{A1K},{A2K}", "thr": "2"}, **ev(f"create_wallet:{W}", NB)), dep=10**24)
print("1. create_wallet:", r1)
if r1[0] != "OK": sys.exit("setup failed: create_wallet")
r2 = call("propose", dict({"name": W, "am": "400000000000000000000000", "rc": C, "pexp": str(EX)}, **ev(f"propose:{W}:{PID}", PID)))
print("2. propose:", r2, "pid:", PID)
if r2[0] != "OK": sys.exit("setup failed: propose")

# kind-37500 approval event by A2 (approver ix 1)
msg = f"expires {EX}.000000000: approve:{W}:{PID}:1 | contract: {C}"
tags = [["contract", C], ["wallet", W], ["proposal", str(PID)], ["approver", "1"], ["action", "approve"]]
cat = int(time.time())
ser = json.dumps([0, A2K, cat, GOV, tags, msg], separators=(",", ":"))
eid = hashlib.sha256(ser.encode()).hexdigest()
sig = sign(A2, bytes.fromhex(eid))
event = {"id": eid, "pubkey": A2K, "created_at": cat, "kind": GOV, "tags": tags, "content": msg, "sig": sig.hex()}
print("3. event id:", eid[:20], "sig:", sig.hex()[:16])

import subprocess
resp = json.loads(subprocess.run(["curl", "-sS", "-X", "POST", f"{WORKER}/ingest",
    "-H", "Content-Type: application/json", "-d", json.dumps({"event": event})],
    capture_output=True, text=True, timeout=30).stdout)
print("4. /ingest:", resp)

print("5. polling on-chain for watcher-submitted approval…")
for i in range(30):
    raw = view("get_proposal", {"name": W, "id": str(PID)})
    try:
        p = json.loads(json.loads(raw)["result"])
        ac = int(p.get("ac", "0"))
        print(f"   [{i*4:>2}s] ac={ac} st={p.get('st')}")
        if ac >= 1: break
    except Exception:
        print("   raw:", raw[:80])
    time.sleep(4)

print("6. direct approve A1:", call("approve", {"name": W, "id": str(PID), "ix": "0", "pubkey_hex": A1K,
      "signature": sign(A1, sha(f"expires {EX}.000000000: approve:{W}:{PID}:0 | contract: {C}".encode())).hex(), "expires_at": str(EX)}))
print("7. execute:", call("execute", dict({"name": W, "id": str(PID)}, **ev(f"execute:{W}:{PID}", NB + 2))))
final = view("get_proposal", {"name": W, "id": str(PID)})
print("8. final:", final[:150])

import subprocess
h = subprocess.run(["curl", "-sS", f"{WORKER}/health"], capture_output=True, text=True, timeout=30).stdout
print("health:", h[:500])
