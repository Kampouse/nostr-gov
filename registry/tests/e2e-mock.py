#!/usr/bin/env python3
"""registry e2e — the treasury registry under near-mock.

Option B verification: register from multiple owners, per-owner listing,
global list + count, dedup, validation and deposit-gating rejections,
owner-only unregister. Signer switching via NEAR_MOCK_SIGNER (predecessor).
"""
import json, os, shutil, subprocess, sys, tempfile

NM = os.environ.get("NEAR_MOCK", shutil.which("near-mock") or "")
HERE = os.path.dirname(os.path.abspath(__file__))
WASM = os.path.abspath(os.path.join(HERE, "..", "target", "registry.wasm"))
REG = "registry.nostrogov.testnet"
DEP = "20000000000000000000000"        # 0.02 NEAR — above the 0.01 gate


def call(state, method, args, signer, dep="", view=False):
    env = {**os.environ, "NEAR_MOCK_SIGNER": signer}
    if dep:
        env["NEAR_MOCK_ATTACH"] = dep
    out = subprocess.run(
        [NM, "cross", state, f"{REG}={WASM}", REG, method, json.dumps(args)]
        + (["--view"] if view else []),
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

    A = "alice.test.near"
    B = "bob.test.near"
    with tempfile.TemporaryDirectory() as td:
        st = os.path.join(td, "reg.bin")
        steps = [
            ("version", call(st, "get_version", {}, A, view=True)),
            ("register alice/gov", call(
                st, "register", {"treasury": f"gov.{A}", "npub": "ab" * 32}, A, dep=DEP)),
            ("register alice/team", call(
                st, "register", {"treasury": f"team.{A}"}, A, dep=DEP)),
            ("register bob/vault", call(
                st, "register", {"treasury": f"vault.{B}"}, B, dep=DEP)),
            ("count=3", call(st, "count", {}, A, view=True)),
            ("list alice", call(st, "list_by_owner", {"account_id": A}, A, view=True)),
            ("list bob", call(st, "list_by_owner", {"account_id": B}, B, view=True)),
            ("dedup rejected", call(
                st, "register", {"treasury": f"gov.{A}"}, A, dep=DEP)),
            ("bad id rejected", call(
                st, "register", {"treasury": "NOT!VALID"}, A, dep=DEP)),
            ("no deposit rejected", call(
                st, "register", {"treasury": f"x.{A}"}, A)),
            ("wrong-owner unregister", call(
                st, "unregister", {"treasury": f"gov.{A}"}, B)),
            ("unregister alice/team", call(
                st, "unregister", {"treasury": f"team.{A}"}, A)),
            ("list alice after", call(
                st, "list_by_owner", {"account_id": A}, A, view=True)),
            ("list_all", call(st, "list_all", {}, A, view=True)),
        ]

        print(f"\n{'step':<24} result")
        print("─" * 96)
        for name, (ok, ret, err) in steps:
            print(f"{name:<24} {'OK' if ok else 'FAIL':<6} {err or ret[:66]}")

        by = {name: res for name, res in steps}
        def r(n): return by[n][1]

        checks = [
            ("version", lambda: '"1"' in r("version")),
            ("register x3", lambda: all(by[n][0] for n in
                ("register alice/gov", "register alice/team", "register bob/vault"))),
            ("count==3", lambda: '"3"' in r("count=3")),
            ("alice list has both", lambda:
                f"gov.{A}" in r("list alice") and f"team.{A}" in r("list alice")),
            ("bob list isolated", lambda:
                f"vault.{B}" in r("list bob") and f"gov.{A}" not in r("list bob")),
            ("dedup ERR", lambda: "ERR_ALREADY_REGISTERED" in by["dedup rejected"][2]),
            ("bad id ERR", lambda: "ERR_BAD_TREASURY_ID" in by["bad id rejected"][2]),
            ("no deposit ERR", lambda: "ERR_STORAGE_DEPOSIT" in by["no deposit rejected"][2]),
            ("wrong-owner ERR", lambda: "ERR_NOT_REGISTERED" in by["wrong-owner unregister"][2]),
            ("unregister ok", lambda: by["unregister alice/team"][0]),
            ("team gone", lambda: f"team.{A}" not in r("list alice after")),
            ("gov kept", lambda: f"gov.{A}" in r("list alice after")),
            ("log keeps row", lambda: f"team.{A}" in r("list_all")),
        ]
        bad = [n for n, c in checks if not c()]
        print("\nVERDICT:", "ALL GREEN" if not bad else f"FAILED: {bad}")
        return 0 if not bad else 1


if __name__ == "__main__":
    sys.exit(main())
