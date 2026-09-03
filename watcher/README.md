# nostr-relay-watcher

Cloudflare Worker (Durable Object) that watches Nostr relays for kind-37500 governance events and submits them to a NEAR multisig treasury contract.

**Trustless relayer** — the watcher only pays gas. The contract verifies the Nostr event signature on-chain (event ID = SHA256(serialization), BIP-340 schnorr over event ID).

## How it works

1. Durable Object opens persistent WebSocket connections to multiple Nostr relays
2. Subscribes to **kind-37500** events tagged `#contract <treasury-contract-id>`
3. Parses the event tags for `wallet`, `proposal`, `approver`, and `action`
4. Forwards the full signed event to the treasury contract via `approve_with_event` or `cancel_vote_with_event`
5. Contract verifies the event on-chain: event ID, schnorr signature, content matches proposal message, pubkey is authorized approver
6. Publishes a result status event back to connected relays

## Features

- **Multi-relay** — connects to multiple relays, auto-reconnects on disconnect (5s backoff)
- **RPC retry** — 3 retries with exponential backoff (1s, 5s, 15s) on retryable errors
- **Contract confirmation** — parses `broadcast_tx_commit` receipts to determine contract success/failure
- **Health endpoint** — per-event status with tx hashes, errors, retry counts
- **Nostr feedback** — publishes kind-1 result events back to relays
- **Dedup** — in-memory event ID set, caps at 10k entries

## Architecture

```
User (nostr-gov app)
  │
  ├─ Direct: signs kind-37500 → publishes to relay → watcher picks up
  │
  └─ Relay toggle: signs kind-37500 → publishes to relay → watcher picks up
                                                                     │
                                                              CF Worker (Durable Object)
                                                                     │
                                                              NEAR RPC (broadcast_tx_commit)
                                                                     │
                                                              Treasury contract verifies event on-chain
```

## Nostr event format

Kind **37500** (governance event), signed by the approver's nostr key:

```json
{
  "kind": 37500,
  "content": "<proposal message from contract>",
  "tags": [
    ["contract", "benchv5.vault.kampy.testnet"],
    ["wallet", "main"],
    ["proposal", "0"],
    ["approver", "0"],
    ["action", "approve"]
  ]
}
```

- `content` must exactly match `proposal.message` from `get_proposal_message()`
- `sig` is BIP-340 schnorr over the event ID (standard NIP-01)
- `action` tag: omit or `"approve"` for approval, `"cancel"` for cancellation

## Setup

### 1. Create a NEAR account for the watcher

```bash
near create-account nw.example.testnet --masterAccount your-account.testnet --initialBalance 2
```

### 2. Deploy

```bash
cd nostr-relay-watcher
npm install
wrangler deploy
```

### 3. Set the signer key secret

The `NEAR_SIGNER_KEY` secret is the **64-char hex seed** (32 bytes) of the watcher account's ed25519 key.

```bash
echo '<64-char-hex-seed>' | wrangler secret put NEAR_SIGNER_KEY
```

### 4. Connect to relays

```bash
curl -X POST https://nostr-relay-watcher.<subdomain>.workers.dev/connect
```

The DO will connect to all relays in `RELAY_URLS` (comma-separated) and start listening.

### 5. Update wrangler.toml vars

```toml
[vars]
NEAR_RPC = "https://rpc.testnet.fastnear.com"
RELAY_URLS = "wss://relay.primal.net,wss://nos.lol,wss://relay.damus.io"
NEAR_ACCOUNT_ID = "nw.example.testnet"
TREASURY_CONTRACT_ID = "your-treasury.testnet"
```

## Endpoints

| Method | Path | Description |
|--------|------|-------------|
| GET | `/health` | Connected relay count, recent events, last error |
| POST | `/connect` | Open relay connections |
| POST | `/disconnect` | Close all relay connections |

## Health response

```json
{
  "connected": 2,
  "relays": [
    { "url": "wss://relay.primal.net", "open": true },
    { "url": "wss://nos.lol", "open": true }
  ],
  "treasury": "benchv5.vault.kampy.testnet",
  "account": "nw82a7e91.testnet",
  "processed": 3,
  "lastError": null,
  "recentEvents": [
    {
      "eventId": "abc123def456",
      "status": "success",
      "method": "approve_with_event",
      "walletName": "main",
      "proposalId": 0,
      "txHash": "7xKf...",
      "error": null,
      "retries": 0
    }
  ]
}
```

## Important notes

- DO instances persist across deploys — use `wrangler delete --force && wrangler deploy` to update DO code
- CF secrets are deleted with the worker — re-set `NEAR_SIGNER_KEY` after delete+redeploy
- `broadcast_tx_commit` is used (not `send_tx`) — verifies signatures before returning
- NEAR signs `ed25519(sha256(tx_bytes))`, not `ed25519(tx_bytes)` — the watcher hashes before signing
- Borsh enum variants are u8 (1 byte), matching the borsh spec and Rust implementation

## Local dev

```bash
wrangler dev
```

## Contract methods called

- `approve_with_event(wallet_name, proposal_id, approver_index, pubkey_hex, event_id_hex, created_at, kind, tags_json, content, sig_hex)`
- `cancel_vote_with_event(same params)`
