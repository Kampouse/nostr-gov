# nostr-relay-watcher

Cloudflare Worker (Durable Object) that watches a Nostr relay for treasury approval events and submits them to NEAR.

**How it works:**

1. Durable Object holds a persistent WebSocket to a Nostr relay
2. Subscribes to kind-1 events tagged `#t nostrgov-<contract-id>`
3. When a matching event arrives, parses the action (APPROVE / CANCEL_VOTE)
4. Submits it to the treasury contract via `approve_with_event` / `cancel_vote_with_event`
5. Contract verifies the Nostr schnorr signature on-chain

**The worker is trustless** — it only pays gas. The contract does all signature verification.

## Setup

### 1. Create NEAR access key on treasury

```bash
# Add a full-access key for the watcher account
near add-key <treasury-contract> <watcher-account-public-key> \
  --permission-type FunctionCall \
  --allowance 0.1 \
  --method-names approve_with_event,cancel_vote_with_event
```

### 2. Configure secrets

```bash
wrangler secret put NEAR_SIGNER_KEY
# paste the ed25519 private key (hex) of the watcher account

wrangler secret put TREASURY_CONTRACT_ID
# e.g. benchv5.vault.kampy.testnet
```

### 3. Deploy

```bash
wrangler deploy
```

### 4. Connect

```bash
curl -X POST https://nostr-relay-watcher.<your-subdomain>.workers.dev/connect
```

Or the DO auto-connects on first request.

## Nostr approval format

A signer publishes a kind-1 event:

```
APPROVE proposal 42 on benchv5.vault.kampy.testnet
```

With tags:
```
["t", "nostrgov-benchv5.vault.kampy.testnet"]
```

The watcher picks it up and submits to the contract. The contract verifies the npub signature.

## Health check

```bash
curl https://nostr-relay-watcher.<your-subdomain>.workers.dev/health
```

## Local dev

```bash
wrangler dev
```