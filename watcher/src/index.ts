/**
 * nostr-relay-watcher
 *
 * Cloudflare Worker + Durable Object.
 * Subscribes to Nostr relay(s) via WebSocket for kind-37500 governance
 * events, verifies they target our treasury contract, and submits them
 * to NEAR via approve_with_event.
 */

// ── Types ─────────────────────────────────────────────────────────────

interface Env {
  NEAR_RPC: string;
  NEAR_ACCOUNT_ID: string;
  RELAY_URLS?: string;
  RELAY_URL: string;
  NEAR_SIGNER_KEY: string;
  TREASURY_CONTRACT_ID: string;
  TREASURY_CONTRACT_IDS?: string; // comma list — supersedes TREASURY_CONTRACT_ID
  RELAY_WATCHER: DurableObjectNamespace;
}

interface NostrEvent {
  id: string;
  pubkey: string;
  created_at: number;
  kind: number;
  tags: string[][];
  content: string;
  sig: string;
}

type EventStatus = "pending" | "submitted" | "success" | "failed";

interface EventRecord {
  eventId: string;
  status: EventStatus;
  method: string;
  walletName: string;
  proposalId: number;
  txHash: string | null;
  error: string | null;
  retries: number;
  createdAt: number;
  updatedAt: number;
}

// Gov envelope: a kind-37500 event whose signed content wraps the FULL
// contract args for propose/execute. The contract re-verifies the admin
// schnorr signature over these bytes on-chain, so tampering with any
// field (amount, recipient...) invalidates the event.
interface GovEnvelope {
  v: 1;
  method: "propose" | "execute";
  contractId: string;
  args: Record<string, unknown>;
}

interface RelayConn {
  ws: WebSocket;
  url: string;
}

class TxResult {
  ok: boolean;
  txHash: string | null;
  error: string | null;
  contractSuccess: boolean | null;

  get recordUpdate() {
    return {
      txHash: this.txHash,
      error: this.error,
      status: (this.ok ? "success" : "failed") as EventStatus,
    };
  }

  constructor(ok: boolean, txHash: string | null, error: string | null, contractSuccess: boolean | null) {
    this.ok = ok;
    this.txHash = txHash;
    this.error = error;
    this.contractSuccess = contractSuccess;
  }
}

// ── Constants ───────────────────────────────────────────────────────────

const GOVERNANCE_KIND = 37500;
const MAX_EVENTS = 500;
const MAX_RETRIES = 3;
const RETRY_DELAYS = [1000, 5000, 15000];

// Multi-contract support: comma list in TREASURY_CONTRACT_IDS supersedes
// TREASURY_CONTRACT_ID. 0-deposit rule: the relayer only ever submits
// calls whose attached NEAR deposit is 0 (approve_with_event, propose,
// execute). Payouts move the TREASURY's balance — the relayer only fronts
// gas (<0.01 N). create_wallet (1.1 N deposit) is NOT relayable: gov
// envelopes whitelist methods, so a relay event can never drain relayer
// funds. The contract re-verifies the admin schnorr sig + nonce on-chain;
// the watcher is a gas puppet that cannot forge or replay anything.
const CONTRACT_IDS = (env: Env): string[] => {
  if (env.TREASURY_CONTRACT_IDS) {
    return env.TREASURY_CONTRACT_IDS.split(",").map((s) => s.trim()).filter(Boolean);
  }
  return [env.TREASURY_CONTRACT_ID];
};

// ── Durable Object ────────────────────────────────────────────────────

export class RelayWatcher {
  private state: DurableObjectState;
  private env: Env;
  private conns: RelayConn[] = [];
  private processedEvents: Set<string> = new Set();
  private reconnectTimers: Map<string, number> = new Map();
  private lastError: string | null = null;
  private eventLog: EventRecord[] = [];

  constructor(state: DurableObjectState, env: Env) {
    this.state = state;
    this.env = env;
    // Re-arm the keepalive alarm on every wake (hibernation-safe).
    void this.state.storage.setAlarm(Date.now() + 10_000).catch(() => {});
  }

  private get relayUrls(): string[] {
    if (this.env.RELAY_URLS) {
      return this.env.RELAY_URLS.split(",").map((s) => s.trim()).filter(Boolean);
    }
    return [this.env.RELAY_URL];
  }

  // ── HTTP fetch ─────────────────────────────────────────────────────

  async fetch(request: Request): Promise<Response> {
    const url = new URL(request.url);

    if (request.method === "GET" && url.pathname === "/health") {
      return Response.json({
        connected: this.conns.filter((c) => c.ws.readyState === WebSocket.OPEN).length,
        relays: this.conns.map((c) => ({ url: c.url, open: c.ws.readyState === WebSocket.OPEN })),
        treasury: CONTRACT_IDS(this.env),
        account: this.env.NEAR_ACCOUNT_ID,
        processed: this.processedEvents.size,
        lastError: this.lastError,
        recentEvents: this.eventLog.slice(-20).map((e) => ({
          eventId: e.eventId.slice(0, 12),
          status: e.status,
          method: e.method,
          walletName: e.walletName,
          proposalId: e.proposalId,
          txHash: e.txHash?.slice(0, 16) ?? null,
          error: e.error,
          retries: e.retries,
        })),
      });
    }

    if (request.method === "POST" && url.pathname === "/connect") {
      this.connectAll();
      return Response.json({ ok: true, relays: this.relayUrls });
    }

    if (request.method === "POST" && url.pathname === "/disconnect") {
      this.disconnectAll();
      return Response.json({ ok: true });
    }

    return new Response("Not found", { status: 404 });
  }

  // ── Relay connections ──────────────────────────────────────────────

  private connectAll() {
    for (const url of this.relayUrls) this.connectRelay(url);
  }

  private disconnectAll() {
    for (const url of this.relayUrls) this.disconnectRelay(url);
  }

  private connectRelay(url: string) {
    const existing = this.conns.find((c) => c.url === url);
    if (existing?.ws.readyState === WebSocket.OPEN) return;

    console.log(`[watcher] connecting to ${url}`);
    const ws = new WebSocket(url);
    const conn: RelayConn = { ws, url };
    this.conns.push(conn);

    ws.addEventListener("open", () => {
      console.log(`[watcher] connected to ${url}, subscribing`);
      // Subscribe to kind-37500 events tagged with any watched contract
      ws.send(JSON.stringify([
        "REQ",
        "gov-watch",
        { kinds: [GOVERNANCE_KIND], "#contract": CONTRACT_IDS(this.env), limit: 100 },
      ]));
    });

    ws.addEventListener("message", (event: MessageEvent) => {
      this.handleRelayMessage(event.data as string, url);
    });

    ws.addEventListener("close", () => {
      console.log(`[watcher] disconnected from ${url}`);
      this.conns = this.conns.filter((c) => c.ws !== ws);
      this.scheduleReconnect(url);
    });

    ws.addEventListener("error", () => {
      console.log(`[watcher] relay error on ${url}`);
    });
  }

  private disconnectRelay(url: string) {
    const timer = this.reconnectTimers.get(url);
    if (timer !== undefined) { clearTimeout(timer); this.reconnectTimers.delete(url); }
    const conn = this.conns.find((c) => c.url === url);
    if (conn) {
      conn.ws.close(1000, "shutdown");
      this.conns = this.conns.filter((c) => c.ws !== conn.ws);
    }
  }

  private scheduleReconnect(url: string) {
    if (this.reconnectTimers.has(url)) return;
    const timer = setTimeout(() => {
      this.reconnectTimers.delete(url);
      this.connectRelay(url);
    }, 5000) as unknown as number;
    this.reconnectTimers.set(url, timer);
  }

  // ── Relay message handling ──────────────────────────────────────────

  private handleRelayMessage(raw: string, relayUrl: string) {
    let msg: unknown;
    try { msg = JSON.parse(raw); } catch { return; }
    if (!Array.isArray(msg)) return;

    const [type] = msg as [string, ...unknown[]];

    if (type === "EVENT") {
      const [, , event] = msg as [string, string, NostrEvent];
      this.handleGovernanceEvent(event, relayUrl);
    } else if (type === "EOSE") {
      console.log(`[watcher] caught up on ${relayUrl}`);
    }
  }

  private async handleGovernanceEvent(event: NostrEvent, relayUrl: string) {
    if (this.processedEvents.has(event.id)) return;
    this.processedEvents.add(event.id);

    if (this.processedEvents.size > 10000) {
      const arr = Array.from(this.processedEvents);
      this.processedEvents = new Set(arr.slice(-5000));
    }

    console.log(`[watcher] event ${event.id.slice(0, 12)} kind=${event.kind} from ${event.pubkey.slice(0, 16)} via ${relayUrl}`);

    // Parse the event to determine contract method
    const parsed = this.parseGovernanceEvent(event);
    if (!parsed) {
      console.log(`[watcher] event does not target our contract or missing required tags, skipping`);
      return;
    }

    const record: EventRecord = {
      eventId: event.id,
      status: "pending",
      method: parsed.method,
      walletName: parsed.walletName,
      proposalId: parsed.proposalId,
      txHash: null,
      error: null,
      retries: 0,
      createdAt: Date.now(),
      updatedAt: Date.now(),
    };
    this.pushEventLog(record);

    const result = await this.submitWithRetry(parsed, event);
    const idx = this.eventLog.findIndex((e) => e.eventId === event.id);
    if (idx >= 0) {
      this.eventLog[idx] = { ...this.eventLog[idx], ...result.recordUpdate, updatedAt: Date.now() };
    }

    console.log(`[watcher] ${result.ok ? "OK" : "FAILED"} ${parsed.method} ${parsed.walletName}#${parsed.proposalId}${result.txHash ? ` tx ${result.txHash.slice(0, 16)}` : ""}${result.error ? ` err: ${result.error}` : ""}`);
    this.lastError = result.error;

    // Publish result back to all connected relays
    this.publishResult(event, result, relayUrl, parsed.contractId);
  }

  // ── Parse governance event ────────────────────────────────────────

  // Two event shapes arrive on kind-37500:
  //  A) approval events — tags [wallet, proposal, approver, action=approve,
  //     contract] → approve_with_event
  //  B) gov envelopes — signed content wraps {v,method,contractId,args} for
  //     propose/execute. Content is schnorr-covered, so the watcher can
  //     trust args match what the admin signed; the contract re-verifies.
  private parseGovernanceEvent(event: NostrEvent): {
    method: string;
    contractId: string;
    walletName: string;
    proposalId: number;
    approverIndex: number;
    envelope: GovEnvelope | null;
  } | null {
    // Must be kind 37500
    if (event.kind !== GOVERNANCE_KIND) return null;

    // Check #contract tag matches ANY watched treasury
    const contractTag = event.tags.find((t) => t[0] === "contract");
    if (!contractTag || !CONTRACT_IDS(this.env).includes(contractTag[1])) return null;
    const contractId = contractTag[1];

    // Shape B: gov envelope in signed content
    if (event.content.startsWith("gov:")) {
      let env: GovEnvelope;
      try {
        env = JSON.parse(event.content.slice(4)) as GovEnvelope;
      } catch {
        return null;
      }
      // Whitelist: only these two methods are ever relayed from envelopes,
      // and create_wallet can never be added (it needs a 1.1 N deposit).
      if (env?.v !== 1 || (env.method !== "propose" && env.method !== "execute")) return null;
      // Envelope contractId must match the signed #contract tag
      if (env.contractId !== contractId) return null;
      const argsName = typeof env.args?.name === "string" ? env.args.name : "";
      const argsId = Number(env.args?.id ?? NaN);
      return {
        method: env.method,
        contractId,
        walletName: argsName,
        proposalId: env.method === "execute" ? argsId : 0,
        approverIndex: 0,
        envelope: env,
      };
    }

    // Shape A: approval event
    const walletTag = event.tags.find((t) => t[0] === "wallet");
    const proposalTag = event.tags.find((t) => t[0] === "proposal");
    if (!walletTag?.[1] || !proposalTag?.[1]) return null;

    const walletName = walletTag[1];
    const proposalId = parseInt(proposalTag[1], 10);
    if (isNaN(proposalId)) return null;

    // Determine approver_index from #approver tag if present, else default 0
    const approverTag = event.tags.find((t) => t[0] === "approver");
    const approverIndex = approverTag?.[1] ? parseInt(approverTag[1], 10) : 0;

    // New TS contract has no cancel path — skip cancel events entirely
    // (submitting would just burn gas on ERR_EVENT_ACTION).
    const actionTag = event.tags.find((t) => t[0] === "action");
    if (actionTag?.[1] === "cancel") return null;
    const method = "approve_with_event";

    return { method, contractId, walletName, proposalId, approverIndex, envelope: null };
  }

  // ── Retry logic ────────────────────────────────────────────────────

  private async submitWithRetry(
    parsed: { method: string; contractId: string; walletName: string; proposalId: number; approverIndex: number; envelope: GovEnvelope | null },
    event: NostrEvent,
  ): Promise<TxResult> {
    for (let attempt = 0; attempt <= MAX_RETRIES; attempt++) {
      const result = await this.submitToNear(parsed, event);
      if (result.ok) return result;

      const isRetryable = result.error && (
        result.error.includes("timeout") ||
        result.error.includes("Timeout") ||
        result.error.includes("500") ||
        result.error.includes("502") ||
        result.error.includes("503") ||
        result.error.includes("Expired") ||
        result.error.includes("nonce") ||
        result.error.includes("send_tx")
      );

      if (!isRetryable || attempt >= MAX_RETRIES) return result;

      const delay = RETRY_DELAYS[Math.min(attempt, RETRY_DELAYS.length - 1)];
      console.log(`[watcher] retryable, attempt ${attempt + 1}/${MAX_RETRIES}, ${delay}ms: ${result.error}`);
      await sleep(delay);
    }
    return new TxResult(false, null, "max retries exceeded", null);
  }

  // ── Submit to NEAR ──────────────────────────────────────────────────

  private async submitToNear(
    parsed: { method: string; contractId: string; walletName: string; proposalId: number; approverIndex: number; envelope: GovEnvelope | null },
    event: NostrEvent,
  ): Promise<TxResult> {
    try {
      const txBody = await this.buildSignedTransaction(parsed, event);

      const res = await fetch(this.env.NEAR_RPC, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ jsonrpc: "2.0", id: Date.now(), method: "broadcast_tx_commit", params: [txBody] }),
      });

      const data = await res.json() as any;

      if (data.error) {
        const cause = data.error.cause?.name || "";
        const txErr = data.error.data?.TxExecutionError?.InvalidTxError || "";
        const detail = txErr || cause || JSON.stringify(data.error.data || "");
        const msg = data.error.message || "Unknown RPC error";
        return new TxResult(false, null, detail ? `${msg}: ${detail}` : msg, null);
      }

      const txHash: string = data.result?.transaction_hash ?? null;
      const execStatus = data.result?.status?.FinalExecutionStatus;
      const receipts = data.result?.receipts_outcome ?? [];

      let contractSuccess: boolean | null = null;
      let contractError: string | null = null;

      for (const receipt of receipts) {
        const outcome = receipt.outcome;
        if (!outcome) continue;
        if (outcome.status?.Failure) {
          contractSuccess = false;
          contractError = outcome.status.Failure?.ActionError?.kind?.FunctionCallError?.ExecutionError
            || outcome.status.Failure?.ActionError?.kind?.FunctionCallError?.WasmTrap
            || JSON.stringify(outcome.status.Failure);
          break;
        }
        if (outcome.status?.SuccessValue !== undefined) {
          if (receipt.executor_id === parsed.contractId) {
            contractSuccess = true;
          }
        }
      }

      const ok = execStatus === "FINAL" || execStatus === "EXECUTED_OPTIMISTIC";
      const error = contractSuccess === false ? contractError : (!ok ? `status: ${execStatus}` : null);

      return new TxResult(ok, txHash, error, contractSuccess);
    } catch (e: unknown) {
      const msg = e instanceof Error ? e.message : String(e);
      return new TxResult(false, null, msg, null);
    }
  }

  // ── Publish result back to relay ─────────────────────────────────────

  private publishResult(event: NostrEvent, result: TxResult, _sourceRelay: string, contractId: string) {
    const status = result.ok ? "success" : "failed";
    const content = JSON.stringify({
      type: "watcher-result",
      sourceEvent: event.id,
      status,
      txHash: result.txHash,
      error: result.error,
      contractSuccess: result.contractSuccess,
    });

    const statusEvent = {
      kind: 1,
      created_at: Math.floor(Date.now() / 1000),
      tags: [
        ["e", event.id],
        ["contract", contractId],
        ["p", result.ok ? "success" : "error"],
      ],
      content,
      pubkey: "watcher",
    };

    const payload = JSON.stringify(["EVENT", statusEvent]);
    for (const conn of this.conns) {
      if (conn.ws.readyState === WebSocket.OPEN) {
        try {
          conn.ws.send(payload);
        } catch { /* ignore */ }
      }
    }
  }

  // ── Event log management ────────────────────────────────────────────

  private pushEventLog(record: EventRecord) {
    this.eventLog.push(record);
    if (this.eventLog.length > MAX_EVENTS) {
      this.eventLog = this.eventLog.slice(-Math.floor(MAX_EVENTS / 2));
    }
  }

  // ── Build signed NEAR transaction ───────────────────────────────────

  private async buildSignedTransaction(
    parsed: { method: string; contractId: string; walletName: string; proposalId: number; approverIndex: number; envelope: GovEnvelope | null },
    event: NostrEvent,
  ): Promise<string> {
    const seed = hexToBytes(this.env.NEAR_SIGNER_KEY);
    if (seed.length !== 32) throw new Error(`Expected 32-byte hex seed, got ${seed.length}`);

    // Derive public key via JWK
    const jwkKey = await crypto.subtle.importKey(
      "pkcs8", buildPkcs8Ed25519(seed),
      { name: "Ed25519", namedCurve: "Ed25519" },
      true, ["sign"],
    );
    const jwk = await crypto.subtle.exportKey("jwk", jwkKey) as JsonWebKey;
    const pubRaw = base64UrlToBytes(jwk.x!);
    const pubKeyStr = "ed25519:" + bytesToBase58(pubRaw);

    const cryptoKey = await crypto.subtle.importKey(
      "pkcs8", buildPkcs8Ed25519(seed),
      { name: "Ed25519", namedCurve: "Ed25519" },
      false, ["sign"],
    );

    // Build contract args — pass through the raw event fields; the TS
    // contract re-serializes the event and verifies the schnorr sig on-chain.
    // cat must be the bare created_at integer so the NIP-01 reconstruction
    // matches what the signer serialized.
    // Gov envelopes instead carry the FULL contract args in their signed
    // content (propose/execute): the payload travels inside the signature,
    // and the contract re-verifies the admin sig over these exact bytes.
    const args = JSON.stringify(parsed.envelope
      ? { ...parsed.envelope.args, ...eventAuthFields(event) }
      : {
          pk: event.pubkey,
          cat: String(event.created_at),
          kind: String(event.kind),
          tags: JSON.stringify(event.tags),
          ct: event.content,
          sig: event.sig,
        });
    const argsB64 = btoa(args);

    // Fetch nonce and block hash
    const [nonceRes, blockRes] = await Promise.all([
      this.rpc("query", { request_type: "view_access_key", finality: "final", account_id: this.env.NEAR_ACCOUNT_ID, public_key: pubKeyStr }),
      this.rpc("block", { finality: "final" }),
    ]);

    const nonce = ((nonceRes as any)?.result?.nonce ?? 0) as number;
    const blockHash = ((blockRes as any)?.result?.header?.hash) as string;
    if (!blockHash) throw new Error("No block hash");

    const tx = serializeTransaction({
      signerId: this.env.NEAR_ACCOUNT_ID,
      publicKey: pubRaw,
      nonce: BigInt(nonce) + 1n,
      receiverId: parsed.contractId,
      actions: [{ type: "FunctionCall" as const, methodName: parsed.method, args: argsB64, gas: 30000000000000n, deposit: 0n }],
      blockHash: base58ToBytes(blockHash),
    });

    // NEAR signs sha256(tx_bytes)
    const txHash = new Uint8Array(await crypto.subtle.digest("SHA-256", tx));
    const signature = new Uint8Array(await crypto.subtle.sign("Ed25519", cryptoKey, txHash));

    const signed = serializeSignedTx(signature, tx);
    return bytesToBase64(signed);
  }

  private async rpc(method: string, params: Record<string, unknown>): Promise<unknown> {
    const res = await fetch(this.env.NEAR_RPC, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ jsonrpc: "2.0", id: 1, method, params }),
    });
    return res.json();
  }

  // ── Lifecycle ────────────────────────────────────────────────────────

  async alarm() {
    // Reconnect any relay that isn't open, then re-arm to keep the loop alive.
    for (const url of this.relayUrls) {
      const conn = this.conns.find((c) => c.url === url);
      if (!conn || conn.ws.readyState !== WebSocket.OPEN) this.connectRelay(url);
    }
    await this.state.storage.setAlarm(Date.now() + 30_000).catch(() => {});
  }
}

// ── Worker entry ────────────────────────────────────────────────────

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const id = env.RELAY_WATCHER.idFromName("singleton");
    return env.RELAY_WATCHER.get(id).fetch(request);
  },
};

// ── Helpers ──────────────────────────────────────────────────────────

function sleep(ms: number): Promise<void> {
  return new Promise((r) => setTimeout(r, ms));
}

// Event-auth fields for gov envelopes (propose/execute): identical shape
// to the FE's eventAuthArgs — pk/ev/sig/kind/tags/ct/cat. The contract's
// verifyOwnerEvent re-serializes these and verifies the admin schnorr sig,
// so any mismatch between envelope args and signed bytes dies on-chain.
function eventAuthFields(event: NostrEvent): Record<string, string> {
  return {
    pk: event.pubkey,
    ev: event.id,
    sig: event.sig,
    kind: String(event.kind),
    tags: JSON.stringify(event.tags),
    ct: event.content,
    cat: String(event.created_at),
  };
}

// ── Borsh serialization ────────────────────────────────────────────────

function u8(n: number): Uint8Array { return new Uint8Array([n]); }
function u32(n: number): Uint8Array { const b = new Uint8Array(4); new DataView(b.buffer).setUint32(0, n, true); return b; }
function u64(n: bigint): Uint8Array { const b = new Uint8Array(8); new DataView(b.buffer).setBigUint64(0, n, true); return b; }
function u128(n: bigint): Uint8Array { const b = new Uint8Array(16); new DataView(b.buffer).setBigUint64(0, n & ((1n << 64n) - 1n), true); new DataView(b.buffer).setBigUint64(8, n >> 64n, true); return b; }
function borshStr(s: string): Uint8Array { const e = new TextEncoder().encode(s); const b = new Uint8Array(4 + e.length); new DataView(b.buffer).setUint32(0, e.length, true); b.set(e, 4); return b; }
function borshBytes(b: Uint8Array): Uint8Array { const h = new Uint8Array(4 + b.length); new DataView(h.buffer).setUint32(0, b.length, true); h.set(b, 4); return h; }

function concat(arrays: Uint8Array[]): Uint8Array {
  const total = arrays.reduce((s, a) => s + a.length, 0);
  const r = new Uint8Array(total);
  let o = 0;
  for (const a of arrays) { r.set(a, o); o += a.length; }
  return r;
}

function serializeAction(action: { type: string; methodName: string; args: string; gas: bigint; deposit: bigint }): Uint8Array {
  switch (action.type) {
    case "FunctionCall": {
      const argsBytes = base64ToBytes(action.args);
      return concat([
        u8(2),
        borshStr(action.methodName),
        borshBytes(argsBytes),
        u64(action.gas),
        u128(action.deposit),
      ]);
    }
    default:
      throw new Error(`Unknown action type: ${action.type}`);
  }
}

function serializeTransaction(tx: {
  signerId: string;
  publicKey: Uint8Array;
  nonce: bigint;
  receiverId: string;
  actions: { type: string; methodName: string; args: string; gas: bigint; deposit: bigint }[];
  blockHash: Uint8Array;
}): Uint8Array {
  const actionBytes = tx.actions.map(serializeAction);
  return concat([
    borshStr(tx.signerId),
    u8(0),
    tx.publicKey,
    u64(tx.nonce),
    borshStr(tx.receiverId),
    tx.blockHash,
    u32(actionBytes.length),
    ...actionBytes,
  ]);
}

function serializeSignedTx(signature: Uint8Array, tx: Uint8Array): Uint8Array {
  return concat([tx, u8(0), signature]);
}

// ── Encoding helpers ──────────────────────────────────────────────────

const BASE58 = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";

function bytesToBase58(bytes: Uint8Array): string {
  let n = 0n;
  for (const b of bytes) n = (n << 8n) | BigInt(b);
  let r = "";
  while (n > 0n) { r = BASE58[Number(n % 58n)] + r; n /= 58n; }
  for (const b of bytes) { if (b === 0) r = "1" + r; else break; }
  return r || "1";
}

function base58ToBytes(s: string): Uint8Array {
  let n = 0n;
  for (const c of s) n = n * 58n + BigInt(BASE58.indexOf(c));
  let pad = 0;
  for (const c of s) { if (c === "1") pad++; else break; }
  const hex = n.toString(16).padStart(2, "0");
  const bytes = new Uint8Array(pad + Math.ceil(hex.length / 2));
  for (let i = 0; i < hex.length; i += 2) bytes[pad + i / 2] = parseInt(hex.slice(i, i + 2), 16);
  return bytes;
}

function base64UrlToBytes(s: string): Uint8Array { const raw = atob(s.replace(/-/g,'+').replace(/_/g,'/')); const bytes = new Uint8Array(raw.length); for(let i=0;i<raw.length;i++) bytes[i]=raw.charCodeAt(i); return bytes; }

function hexToBytes(s: string): Uint8Array {
  const bytes = new Uint8Array(s.length / 2);
  for (let i = 0; i < s.length; i += 2) bytes[i / 2] = parseInt(s.slice(i, i + 2), 16);
  return bytes;
}

function base64ToBytes(s: string): Uint8Array {
  const binary = atob(s);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  return bytes;
}

function bytesToBase64(bytes: Uint8Array): string {
  let binary = "";
  for (const b of bytes) binary += String.fromCharCode(b);
  return btoa(binary);
}

function buildPkcs8Ed25519(seed: Uint8Array): Uint8Array {
  const algoId = new Uint8Array([0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70]);
  const version = new Uint8Array([0x02, 0x01, 0x00]);
  const innerOctet = new Uint8Array(2 + 32);
  innerOctet[0] = 0x04; innerOctet[1] = 32;
  innerOctet.set(seed, 2);
  const outerOctet = new Uint8Array(2 + innerOctet.length);
  outerOctet[0] = 0x04; outerOctet[1] = innerOctet.length;
  outerOctet.set(innerOctet, 2);
  const inner = concat([version, algoId, outerOctet]);
  const der = new Uint8Array(2 + inner.length);
  der[0] = 0x30; der[1] = inner.length;
  der.set(inner, 2);
  return der;
}
