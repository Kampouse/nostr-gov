# nostr-gov-ts — the lisp-rlm TypeScript contract

The governance contract in the **lisp-rlm TS dialect**, compiled to NEAR
wasm by the [lisp-rlm](https://github.com/Kampouse/lisp-rlm) toolchain.

NIP-01 event-governed multisig wallet: nostr events as auth, BIP-340
schnorr signatures, t-of-n approver rotation, typed proposals
(payout / rotate / unpause), NEAR + NEP-141 payouts, sliding 64-slot
nonce window with jump-on-high.

**This directory is the canonical home of the TS contract.** The lisp
twin (differential oracle) lives in lisp-rlm at
`projects/nostr-gov-lisp/src/main.lisp`; keep the two in lockstep —
behavioral changes land in both, proven by differential tests.

## Layout

```
src/main.ts          the contract (export function = contract method, get_* = view)
types/lisp-rlm.d.ts  dialect reference types (editor + typecheck)
build.sh             TS → wasm (lowers to lisp, then the NEAR pipeline)
tests/               bip340.py · gen-vectors.py · run-gauntlet.sh
near.json            deploy config (gov2.kampy.testnet @ testnet)
```

## Build

Requires [lisp-rlm](https://github.com/Kampouse/lisp-rlm) (public):
`git clone git@github.com:Kampouse/lisp-rlm.git ../lisp-rlm` or set
`LISP_RLM_ROOT`. First run builds the compiler (cargo, ~2 min).

```bash
./build.sh                       # → target/nostr-gov-ts.wasm
```

## Verify

```bash
tests/run-gauntlet.sh            # builds + drives 68 governance vectors
```

Vectors cover: wallet create/rotate, typed proposals (payout/appr/unp),
threshold enforcement, pause/unpause, nonce replay + jump-on-high,
expiry gates, schnorr event-auth (official BIP-340 vectors in
`tests/bip340.py`), and attack cases. Differential twins (lisp vs TS)
run from the lisp-rlm repo: `projects/nostr-gov-lisp/tests/diff-ts*.sh`.

CI: `.github/workflows/contract-ts.yml` builds and gauntlets every push.

## Deploy

```bash
near deploy --useFile target/nostr-gov-ts.wasm --accountId gov2.kampy.testnet
```
