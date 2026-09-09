# nostr-gov treasury registry

The on-chain answer to "which treasuries exist?" NEAR has no sub-account
enumeration and plain accounts hold no storage, so discovery was
localStorage-only (new device = empty app) until an indexer scan patched it.
The registry replaces both as the **authoritative** source:

```
deploy treasury (UI batch) ──register──▶ registry.nostrogov.testnet
                                             │
any device ◀────── list_by_owner(account) ───┘
```

## Interface

| method | kind | description |
|---|---|---|
| `register { treasury, npub? }` | call | predecessor becomes owner; 0.01 Ⓝ storage gate (`depositGte(0, 542)` ≈ 542×2⁶⁴ yocto); dedup + id validation |
| `unregister { treasury }` | call | owner-only; drops the list entry (append-only log row stays) |
| `list_by_owner { account_id }` | view | comma-joined treasuries of that owner |
| `list_all` | view | `treasury\|owner\|npub` rows, newline-joined |
| `count` | view | total registrations |
| `get_version` | view | `"1"` |

## Trust model

Public, keyless, censor-proof: anyone may register (storage staking is the
spam gate — same economics as NEAR itself), only the owner may unregister.
Invalidation is per-owner only; there is no admin.

## Build & test (offline, near-mock)

```bash
./build.sh                    # compile + wasm-opt + sync ../public/registry.wasm
python3 tests/e2e-mock.py     # multi-owner register/list/dedup/reject matrix — ALL GREEN
```

## Deploy (one-time, needs a funded testnet account)

```bash
./scripts/deploy-registry.sh registry.nostrogov.testnet <your-account>.testnet
```

Then confirm `REGISTRY_CONTRACT` in `src/lib/constants.ts` matches.

## UI integration

- **Create treasury**: the same atomic batch appends
  `FunctionCall(REGISTRY_CONTRACT, register, 0.02 Ⓝ)` — registry and
  deployment cannot drift. If the registry isn't deployed yet, creation
  proceeds without registration (warned in a toast).
- **Discovery**: `list_by_owner` first; the NearBlocks indexer scan remains
  only as a legacy fallback for pre-registry treasuries; localStorage is a
  cache.

## Notes

- `depositGte(lo, hi)` is ONE u128 threshold split (lo64, hi64) — NOT
  (base, per-byte). 542×2⁶⁴ ≈ 0.009998 Ⓝ. Also: its result is
  boolean-tagged — use `if (!near.depositGte(...))`, never `!== 1`
  (bool≠num is always-true and inverts the gate). Both bitten, both fixed.
- Mainnet-ready: treasury ids ending `.near` or `.testnet` are accepted.
