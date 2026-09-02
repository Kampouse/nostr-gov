/**
 * constants.ts — Relays, KV, RPC, contract addresses
 */

// ── Relays ──
export const NIP46_RELAYS = [
  "wss://relay.powr.build",
  "wss://relay.primal.net",
  "wss://relay.nip46.com",
  "wss://nos.lol",
];

export const READ_RELAYS = [
  "wss://relay.primal.net",
  "wss://nos.lol",
  "wss://relay.damus.io",
  "wss://relay.snort.social",
  "wss://relay.nostr.net",
];

export const WRITE_RELAYS = [
  "wss://relay.damus.io",
  "wss://nos.lol",
  "wss://relay.primal.net",
];

const PRIMAL_CACHE = "wss://cache2.primal.net/v1";

// ── NEAR ──
export const KV_ACCOUNT = "contextual.near";
export const KV_ACCOUNTS = [KV_ACCOUNT];
export const FASTNEAR_KV_API = "https://kv.testnet.fastnear.com";
export const NEAR_RPC = "https://rpc.testnet.fastnear.com";
export const NEAR_NETWORK_ID = "testnet";

// ── nostr-msig (testnet) ──
export const DEFAULT_TREASURY = "benchv5.vault.kampy.testnet";

// ── Relayer watcher ──
export const RELAYER_WATCHER_URL = "https://nostr-relay-watcher.kj95hgdgnn.workers.dev";
export const RELAYER_RELAYS = ["wss://relay.primal.net", "wss://nos.lol", "wss://relay.damus.io"];
