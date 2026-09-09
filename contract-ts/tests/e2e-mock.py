#!/usr/bin/env python3
"""e2e-mock.py — the full signed-governance lifecycle against near-mock.

Offline twin of e2e-fresh.py: same story (init → create_wallet → propose →
2-of-2 approve → execute), same signing, but no testnet — the contract runs
under near-mock against a local state file. Every signature is verified
in-contract by the stitched BIP-340 lib, exactly as on-chain.

Requires:
  - near-mock on PATH or NEAR_MOCK=path (cargo install near-mock; 0.1.2+)
  - the compiled contract: ../target/nostr-gov-ts.wasm (run build.sh first)
"""
import json, os, shutil, subprocess, sys, tempfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from bip340 import sign, event_id, sha, i2b, mul, b2i  # noqa: E402

HERE = os.path.dirname(os.path.abspath(__file__))
WASM = os.environ.get(
    "NEAR_GOV_WASM",
    os.path.abspath(os.path.join(HERE, "..", "target", "nostr-gov-ts.wasm")))

NM = os.environ.get("NEAR_MOCK", shutil.which("near-mock") or "")
GOV = 37500
C = "gov.test.near"                       # the mock account the contract runs as
NOW_S = 1787000000                        # NEAR_MOCK_NOW base (seconds)
NOW_NS = NOW_S * 10**9
EX = NOW_NS + 3600 * 10**9                # 1h into the mock clock
NB = NOW_S                                # nonce base = the run's 'block'

OSK = bytes([0xEE] * 32); OPK = i2b(mul(b2i(OSK))[0]).hex()   # admin/owner
A1 = bytes([0xA1] * 32); A1K = i2b(mul(b2i(A1))[0]).hex()     # approver ix=0
A2 = bytes([0xA2] * 32); A2K = i2b(mul(b2i(A2))[0]).hex()     # approver ix=1
W = "team"
sent = lambda s: s.replace('"', "~")      # sentinel transport (e2e convention)


def ev(action, nonce):
    """Signed admin governance event.
    Tags travel as real JSON (no sentinel) — JSON.stringify and the
    contract's naive concat produce identical bytes for a JSON array.
    Content uses sentinel (~ for ") for gov envelopes; plain text for
    admin actions. We sign over the same bytes the contract will hash."""
    tags = (f'[["t","nostr-gov"],["action","{action}"],["nonce","{nonce}"],'
            f'["expires","{EX}"],["contract","{C}"]]')
    ct = "nostr-gov owner action"
    eid = event_id(OPK, 1, GOV, json.loads(tags), ct)
    ser = f'[0,"{OPK}",1,{GOV},{tags},"{ct}"]'
    return {"pk": OPK, "ev": eid, "cat": "1", "kind": str(GOV),
            "tags": tags, "ct": ct, "sig": sign(OSK, sha(ser.encode())).hex()}


class Chain:
    def __init__(self, state):
        self.state = state

    def call(self, method, args, view=False, dep=0):
        env = {**os.environ, "NEAR_MOCK_NOW": str(NOW_S)}
        if dep:
            env["NEAR_MOCK_ATTACH"] = str(dep)
        out = subprocess.run(
            [NM, "cross", self.state, f"{C}={WASM}", C, method,
             json.dumps(args)] + (["--view"] if view else []),
            capture_output=True, text=True, env=env)
        blob = out.stdout + out.stderr
        ok = out.returncode == 0 and "LOG: ERR_" not in blob
        ret = next((l[2:].strip() for l in blob.splitlines() if l.startswith("📄 ")), "")
        err = next((l.split("LOG: ")[1].strip() for l in blob.splitlines()
                    if "LOG: ERR_" in l), "")
        return ok, ret, err


def main():
    if not NM:
        print("✗ near-mock not found (set NEAR_MOCK= or cargo install near-mock)")
        return 1
    if not os.path.exists(WASM):
        print(f"✗ {WASM} missing — run ../build.sh first")
        return 1

    with tempfile.TemporaryDirectory() as td:
        ch = Chain(os.path.join(td, "mock.bin"))
        pid = str(NB + 1)  # proposal id = the propose event nonce (contract design)

        steps = [
            ("init", ch.call("init", {"npub": OPK})),
            ("get_version", ch.call("get_version", {}, view=True)),
            ("create_wallet", ch.call(
                "create_wallet",
                dict({"name": W, "pks": f"{A1K},{A2K}", "thr": "2"},
                     **ev(f"create_wallet:{W}", NB)),
                dep=2 * 10**24)),                       # storage staking
            ("wallet_count", ch.call("get_wallet_count", {}, view=True)),
            ("propose", ch.call(
                "propose",
                dict({"name": W, "am": "400000000000000000000000",
                      "rc": C, "pexp": str(EX)},
                     **ev(f"propose:{W}:{NB+1}", NB + 1)))),
            ("proposal_ids", ch.call("get_proposal_ids", {"name": W}, view=True)),
        ]
        # approval messages bind the APPROVER INDEX (ix) — sign per-slot
        for ix, (k, pk) in enumerate([(A1, A1K), (A2, A2K)]):
            msg = f"expires {EX}.000000000: approve:{W}:{pid}:{ix} | contract: {C}"
            steps.append((f"approve ix={ix}", ch.call(
                "approve", {"name": W, "id": pid, "ix": str(ix),
                            "pubkey_hex": pk,
                            "signature": sign(k, sha(msg.encode())).hex(),
                            "expires_at": str(EX)})))
        steps.append(("execute", ch.call(
            "execute", dict({"name": W, "id": pid},
                            **ev(f"execute:{W}:{pid}", NB + 2)))))
        steps.append(("final_proposal",
                      ch.call("get_proposal", {"name": W, "id": pid}, view=True)))

        print(f"\n{'step':<18} result")
        print("─" * 96)
        for name, (ok, ret, err) in steps:
            print(f"{name:<18} {'OK' if ok else 'FAIL':<6} {err or ret[:70]}")

        need_ok = ["init", "create_wallet", "propose", "approve ix=0",
                   "approve ix=1", "execute"]
        by_name = {n: ok for n, (ok, _, _) in steps}
        final = dict((n, r) for n, (_, r, _) in steps)["final_proposal"]
        # the proposal is a JSON string nested inside the view's JSON —
        # decode once and check the field, backslash-proof
        executed = False
        try:
            executed = json.loads(json.loads(final)["result"])["st"] == "executed"
        except Exception:
            executed = 'executed' in final.replace(chr(92) + chr(34), chr(34)) and '"st"' in final
        bad = [n for n in need_ok if not by_name[n]]
        verdict = "ALL GREEN" if not bad and executed else f"FAILED: {bad or 'final state'}"
        print("\nVERDICT:", verdict)
        return 0 if verdict == "ALL GREEN" else 1


if __name__ == "__main__":
    sys.exit(main())
