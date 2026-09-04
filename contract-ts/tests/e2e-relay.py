#!/usr/bin/env python3
# Relay-path E2E: fresh proposal -> publish signed kind-37500 to the real
# Nostr relays (wss) -> expect the CF watcher's subscription to pick it up
# and submit on-chain. Reports per-relay OK/reject reasons.
import asyncio, base64, hashlib, json, os, subprocess, sys, time, urllib.request
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "tests"))
from bip340 import b2i, event_id, i2b, mul, sha, sign
from nacl.signing import SigningKey
import websockets

RPC = "https://rpc.testnet.fastnear.com"
C = "nostrgove2e.testnet"
WORKER = "https://nostr-relay-watcher.kj95hgdgnn.workers.dev"
RELAYS = ["wss://relay.primal.net", "wss://nos.lol", "wss://relay.damus.io"]
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

def bstr(s):
    b = s.encode(); return len(b).to_bytes(4, "little") + b
def bbytes(b):
    return len(b).to_bytes(4, "little") + b
_id = [0]
def rpc(m, p):
    _id[0] += 1
    for _ in range(4):
        try:
            req = urllib.request.Request(RPC, data=json.dumps({"jsonrpc": "2.0", "id": _id[0], "method": m, "params": p}).encode(), headers={"Content-Type": "application/json"})
            return json.loads(urllib.request.urlopen(req, timeout=90).read())
        except Exception:
            time.sleep(3)
    raise RuntimeError("rpc retries exhausted")
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
            if "InvalidNonce" in json.dumps(r["error"]): time.sleep(2); continue
            return ("RPC_ERROR", json.dumps(r["error"])[:150])
        logs = []; fail = None
        for rcpt in r.get("result", {}).get("receipts_outcome", []):
            logs += rcpt["outcome"].get("logs", [])
            st = rcpt["outcome"]["status"]
            if "Failure" in st: fail = st["Failure"]; break
        errs = [l for l in logs if "ERR_" in l]
        if fail and not errs: return ("TRAP", json.dumps(fail)[:150])
        return (errs[0].split(":")[0] if errs else "OK", r.get("result", {}).get("transaction_hash", "")[:18])
    return ("RPC_ERROR", "nonce exhausted")
def view(m, a):
    r = rpc("query", {"request_type": "call_function", "account_id": C, "method_name": m,
                      "args_base64": base64.b64encode(json.dumps(a).encode()).decode(), "finality": "final"})
    return bytes(r["result"]["result"]).decode()

TS = int(time.time() * 1e9); EX = TS + 3600 * 10**9
NB = int(time.time()) % 100_000_000 + 1_000_000
W = f"rly{NB % 10000}"; PID = NB + 1
OSK = bytes([0xEE] * 32); OPK = i2b(mul(b2i(OSK))[0]).hex()
A1 = bytes([0xA1] * 32); A1K = i2b(mul(b2i(A1))[0]).hex()
A2 = bytes([0xA2] * 32); A2K = i2b(mul(b2i(A2))[0]).hex()
def ev(action, nonce):
    tags = f'[["t","nostr-gov"],["action","{action}"],["nonce","{nonce}"],["expires","{EX}"],["contract","{C}"]]'
    ct = "nostr-gov owner action"
    eid = event_id(OPK, 1, GOV, json.loads(tags), ct)
    ser = f'[0,"{OPK}",1,{GOV},{tags},"{ct}"]'
    return {"pk": OPK, "ev": eid, "cat": "1", "kind": str(GOV), "tags": tags.replace('"', "~"), "ct": ct, "sig": sign(OSK, sha(ser.encode())).hex()}

print("1. create_wallet:", call("create_wallet", dict({"name": W, "pks": f"{A1K},{A2K}", "thr": "2"}, **ev(f"create_wallet:{W}", NB)), dep=10**24))
print("2. propose:", call("propose", dict({"name": W, "am": "400000000000000000000000", "rc": C, "pexp": str(EX)}, **ev(f"propose:{W}:{PID}", PID))), "pid:", PID)

msg = f"expires {EX}.000000000: approve:{W}:{PID}:1 | contract: {C}"
tags = [["t", "nostr-gov"], ["contract", C], ["wallet", W], ["proposal", str(PID)], ["approver", "1"], ["action", "approve"]]  # t tag: relay-indexed (see watcher REQ fix)
cat = int(time.time())
ser = json.dumps([0, A2K, cat, GOV, tags, msg], separators=(",", ":"))
eid = hashlib.sha256(ser.encode()).hexdigest()
event = {"id": eid, "pubkey": A2K, "created_at": cat, "kind": GOV, "tags": tags, "content": msg, "sig": sign(A2, bytes.fromhex(eid)).hex()}
print("3. event:", eid[:24])

async def publish():
    results = {}
    for url in RELAYS:
        try:
            async with websockets.connect(url, open_timeout=10) as ws:
                await ws.send(json.dumps(["EVENT", event]))
                deadline = time.time() + 8
                while time.time() < deadline:
                    try:
                        resp = json.loads(await asyncio.wait_for(ws.recv(), timeout=3))
                    except asyncio.TimeoutError:
                        break
                    if resp[0] == "OK" and resp[1] == eid:
                        results[url] = (resp[2], resp[3] if len(resp) > 3 else "")
                        if resp[2]: break
                    elif resp[0] == "NOTICE":
                        results[url] = ("NOTICE", str(resp[1:])[:80])
        except Exception as e:
            results[url] = ("CONN_ERR", str(e)[:80])
    return results

res = asyncio.run(publish())
print("4. relay responses:")
for url, (ok, reason) in res.items():
    print(f"   {'✅' if ok is True else '❌'} {url}: ok={ok} {reason}")

print("5. polling on-chain for watcher-submitted approval (relay path)…")
for i in range(20):
    raw = view("get_proposal", {"name": W, "id": str(PID)})
    try:
        p = json.loads(json.loads(raw)["result"])
        print(f"   [{i*4:>2}s] ac={p.get('ac')} st={p.get('st')}")
        if int(p.get("ac", "0")) >= 1:
            print("   🎉 RELAY PATH WORKS — watcher picked it up from the relay subscription")
            break
    except Exception:
        print("   raw:", raw[:60])
    time.sleep(4)
open("/tmp/rly_state.json", "w").write(json.dumps({"W": W, "PID": PID, "EX": EX, "A1K": A1K}))
h = subprocess.run(["curl", "-sS", f"{WORKER}/health"], capture_output=True, text=True, timeout=30).stdout
print("6. health tail:", h[-320:])
