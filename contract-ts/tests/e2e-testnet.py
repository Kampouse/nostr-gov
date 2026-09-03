# Live testnet e2e — full governance story against a deployed nostr-gov
# contract. Requires: near-cli-rs (near) with a funded key for the
# contract account in ~/.near-credentials/testnet/, python3 + pynacl.
#
#   NG_CONTRACT=gov.anon372340656.testnet python3 tests/e2e-testnet.py
#
# Story: create wallet (event-auth) → registry views → propose payout →
# direct approve (schnorr msg sig) → gasless approve_with_event
# (kind-37500) → execute → real funds move. Verified 2026-09-03.
# NOTE: wallet names/proposal ids are per-run unique-ify by editing the
# constants below (nonce 100/101/102 already consumed on the default
# contract).

import sys, json, subprocess, time
sys.path.insert(0, 'tests')
from bip340 import i2b, mul, b2i, sign, sha, event_id
def H(b): return sha(b).hex() if isinstance(b, bytes) else b
import bip340

import os
C = os.environ.get("NG_CONTRACT", "gov.anon372340656.testnet")
PAYER = os.environ.get("NG_PAYER", C)
TS = int(time.time() * 1e9)  # run-scoped; nonce args must be unique per run
EXPIRES = TS + 3600_000_000_000
OSK = bytes([0xEE] * 32)
OPK = i2b(mul(b2i(OSK))[0]).hex()
APR1 = bytes([0xA1] * 32); APR1_PK = i2b(mul(b2i(APR1))[0]).hex()
APR2 = bytes([0xA2] * 32); APR2_PK = i2b(mul(b2i(APR2))[0]).hex()
GOV = 37500

def ev(action, nonce, sk=OSK, pk=OPK, exp=EXPIRES):
    tags = (f'[["t","nostr-gov"],["action","{action}"],["nonce","{nonce}"],'
            f'["expires","{exp}"],["contract","{C}"]]')
    ct = "nostr-gov owner action"
    ser = f'[0,"{pk}",1,{GOV},{tags},"{ct}"]'
    eid = event_id(pk, 1, GOV, json.loads(tags), ct)
    return {"pk": pk, "ev": eid, "cat": "1", "kind": str(GOV),
            "tags": tags, "ct": ct, "sig": sign(sk, sha(ser.encode())).hex()}

def approval_event(sk, pk, wallet, pid, ix, exp=EXPIRES, action="approve"):
    msg = f"expires {exp}.000000000: approve:{wallet}:{pid}:{ix} | contract: {C}"
    tags = (f'[["contract","{C}"],["wallet","{wallet}"],["proposal","{pid}"],'
            f'["approver","{ix}"],["action","{action}"]]')
    ser = f'[0,"{pk}",1,{GOV},{tags},"{msg}"]'
    eid = event_id(pk, 1, GOV, json.loads(tags), msg)
    return {"pk": pk, "ev": eid, "cat": "1", "kind": str(GOV),
            "tags": tags, "ct": msg, "sig": sign(sk, sha(ser.encode())).hex()}

def call(method, args, deposit="0"):
    a = json.dumps(args, separators=(",", ":"))
    cmd = ["near", "call", C, method, a, "--accountId", C,
           "--network-id", "testnet", "--gas", "300000000000000",
           "--depositYocto", deposit]
    r = subprocess.run(cmd, capture_output=True, text=True, input="y\n", timeout=120)
    out = r.stdout + r.stderr
    errs = [l for l in out.split("\n") if "ERR_" in l]
    if r.returncode != 0 and not errs:
        # find a real failure line
        fails = [l for l in out.split("\n") if "execution error" in l.lower() or "Action failed" in l]
        if fails: errs = [fails[0][:60]]
    return ("OK" if not errs else errs[0].strip()[:70]), ""

def view(method, args):
    a = json.dumps(args, separators=(",", ":"))
    cmd = ["near", "view", C, method, a, "--network-id", "testnet"]
    r = subprocess.run(cmd, capture_output=True, text=True, timeout=60)
    out = r.stdout
    i = out.find("[")
    if i < 0:
        i = out.find('"')
    return out[i:i+80].replace("\n", " ") if i >= 0 else out[-80:]

steps = []
# 1. init owner
steps.append(("init", call("init", {"npub": OPK})[:1]))
# 2. version
steps.append(("get_version", view("get_version", {})))
# 3. create wallet (thr 2, two approvers)
steps.append(("create_wallet", call("create_wallet", dict({"name": "prod", "pks": f"{APR1_PK},{APR2_PK}", "thr": "2"}, **ev("create_wallet:prod", 100)), deposit="1000000000000000000000000")))
# 4. registry views
steps.append(("get_wallet_count", view("get_wallet_count", {})))
steps.append(("get_wallet_name", view("get_wallet_name", {"i": "0"})))
steps.append(("get_wallet", view("get_wallet", {"name": "prod"})))
# 5. propose payout to a REAL account
steps.append(("propose", call("propose", dict({"name": "prod", "am": "1000000000000000000000000", "rc": PAYER, "pexp": str(EXPIRES)}, **ev("propose:prod:101", 101)))))
steps.append(("get_proposal_ids", view("get_proposal_ids", {"name": "prod"})))
steps.append(("get_proposal", view("get_proposal", {"name": "prod", "id": "101"})))
steps.append(("get_proposal_message", view("get_proposal_message", {"name": "prod", "id": "101", "ix": "0", "exp": str(EXPIRES)})))
# 6. direct approve by approver 0 (schnorr over message)
msg = f"expires {EXPIRES}.000000000: approve:prod:101:0 | contract: {C}"
steps.append(("approve direct", call("approve", {"name": "prod", "id": "101", "ix": "0", "pubkey_hex": APR1_PK, "signature": sign(APR1, sha(msg.encode())).hex(), "expires_at": str(EXPIRES)})))
# 7. gasless approve_with_event by approver 1 (kind-37500)
steps.append(("approve_with_event", call("approve_with_event", approval_event(APR2, APR2_PK, "prod", "101", "1"))))
steps.append(("get_proposal after", view("get_proposal", {"name": "prod", "id": "101"})))
# 8. execute payout
steps.append(("execute", call("execute", dict({"name": "prod", "id": "101"}, **ev("execute:prod:101", 102)))))
steps.append(("get_proposal final", view("get_proposal", {"name": "prod", "id": "101"})))

print(f"\n{'step':<22} result")
print("─" * 100)
for s in steps:
    name = s[0]; val = s[1] if len(s) > 1 else ""
    if isinstance(val, tuple):
        r = val[0]; extra = val[1] if len(val) > 1 else ""
    else:
        r = val; extra = ""
    print(f"{name:<22} {str(r)[:60]:<62} {str(extra)[:40]}")
