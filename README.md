# nostr-gov

Nostr-npub multisig treasury on NEAR. Govern wallets with schnorr signatures from your nostr key — no NEAR account needed to approve.

**Contract**: `clear-msig` — on-chain schnorr verification, NIP-46 compatible event-based approvals, relayer support.
**App**: React + Vite, dark theme, lucide icons.

## Architecture

```
Approver (nostr key)
  │
  ├─ nsec: schnorr sign proposal message → call contract directly (NEAR wallet)
  ├─ NIP-46: bunker signs kind-37500 event → call contract via NEAR wallet
  ớ─ NIP-46 + Relayer: bunker signs kind-37500 event → publish to relay → watcher submits
  │
  └─ Contract verifies:
      - Event ID = SHA256(JSON serialization)
      - Schnorr(pubkey, sig, event_id)
      - Content matches proposal.message
      - Pubkey is in intent.nostr_approvers[index]
```

## Features

- **Create treasuries** — deploys `<name>.<account>.testnet` with clear-msig
- **Create wallets** — each wallet has its own intents, proposals, and balance
- **Add intents** — define proposers, approvers, approval thresholds, timelocks
- **Propose** — owner proposes actions (transfer, deposit, cross-contract call)
- **Approve** — three paths:
  - **nsec**: direct schnorr sign, cheapest gas
  - **NIP-46**: bunker signs kind-37500 event, submit via NEAR wallet
  - **Relayer**: bunker signs kind-37500 event, publish to relays, watcher submits (no gas for user)
- **Cancel vote** — retract approval (relayer mode)
- **Execute** — owner executes approved proposals after timelock
- **Nostr identity** — login via npub, nsec, NIP-07 extension, or NIP-46 bunker

## Relayer mode

When the ⚡ Relayer toggle is on:

1. App signs a kind-37500 event with the proposal message
2. Publishes it to Nostr relays (primal, nos.lol, damus)
3. The [nostr-relay-watcher](https://github.com/Kampouse/nostr-relay-watcher) CF Worker picks it up
4. Watcher submits to the contract, paying gas from its own account
5. Contract verifies the event signature on-chain

No NEAR wallet connection required. No gas cost to the user.

### Kind-37500 event tags

```json
["contract", "benchv5.vault.kampy.testnet"],
["wallet", "main"],
["proposal", "0"],
["approver", "0"],
["action", "approve"]
```

## Setup

### Prerequisites

- Node 22+
- A NEAR testnet account with some Ⓝ
- A nostr key (nsec or NIP-46 bunker)

### Install

```bash
cd nostr-gov
npm install
```

### Contract

The contract is in `contract/`. Build with `cargo build --target wasm32-unknown-unknown --release`.

Place the WASM at `public/clear_msig.wasm` for the app to deploy it.

### Env

Copy `.env.example` to `.env` if needed. The app uses `NEAR_RPC` and `FASTNEAR_KV_API` from `src/lib/constants.ts`.

### Dev

```bash
npm run dev
```

### Build

```bash
npm run build
```

### Typecheck

```bash
npx tsc -b
```

## Contract methods

| Method | Description |
|--------|-------------|
| `new(owner_npubs)` | Initialize with owner nostr npubs |
| `create_wallet(name, signature, expires_at, nonce)` | Create a multisig wallet (owner only) |
| `propose(wallet_name, intent_index, param_values, expires_at, nonce, signature)` | Propose an action (owner) |
| `approve(wallet_name, proposal_id, approver_index, pubkey_hex, signature, expires_at)` | Approve via direct schnorr |
| `approve_with_event(wallet_name, proposal_id, approver_index, pubkey_hex, event_id_hex, created_at, kind, tags_json, content, sig_hex)` | Approve via signed nostr event |
| `cancel_vote_with_event(...)` | Cancel approval via signed nostr event |
| `execute(wallet_name, proposal_id, signature, expires_at, nonce)` | Execute approved proposal (owner) |
| `quick_execute(wallet_name, intent_index, param_values, expires_at, nonce, signature)` | Solo-user propose+approve+execute in one tx |

## Project structure

```
src/
  lib/
    binding.ts       NEAR↔Nostr binding via FastNear KV
    constants.ts     Relays, RPC, contract addresses
    near.ts          NEAR RPC read methods (raw fetch)
    nostr.ts         Relay pool, profiles, publish
    schnorr.ts       BIP-340 schnorr sign + event builders
    types.ts         Shared types
  hooks/
    useAuth.tsx      Nostr auth (nsec, NIP-07, NIP-46)
    useNearWallet.tsx NEAR wallet connection
  pages/
    GovernancePage.tsx  Treasury + wallet + proposal management
    FeedPage.tsx         Nostr governance feed
    IdentityPage.tsx     Profile + binding management
  components/
    Layout.tsx
    LoginScreen.tsx
contract/
  src/lib.rs         clear-msig contract (Rust)
```