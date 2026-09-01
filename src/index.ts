/**
 * nostr-relay-watcher
 *
 * Cloudflare Worker + Durable Object.
 * Subscribes to a Nostr relay via WebSocket, watches for approval/cancel
 * events tagged with the treasury contract ID, and submits them to NEAR.
 */

// ── Types ─────────────────────────────────────────────────────────────

interface Env {
  NEAR_RPC: string;
  NEAR_ACCOUNT_ID: string;
  RELAY_URL: string;
  SUBSCRIPTION_TAG: string;
  NEAR_SIGNER_KEY: string; // ed25519:<base64-secretKey>
  TREASURY_CONTRACT_ID: string;
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

// ── Durable Object ────────────────────────────────────────────────────

export class RelayWatcher {
  private state: DurableObjectState;
  private env: Env;
  private ws: WebSocket | null = null;
  private processedEvents: Set<string> = new Set();
  private reconnectTimer: number | null = null;

  constructor(state: DurableObjectState, env: Env) {
    this.state = state;
    this.env = env;
  }

  // HTTP fetch
  async fetch(request: Request): Promise<Response> {
    const url = new URL(request.url);

    if (request.method === "GET" && url.pathname === "/health") {
      return Response.json({
        connected: this.ws !== null && this.ws.readyState === WebSocket.OPEN,
        treasury: this.env.TREASURY_CONTRACT_ID,
        relay: this.env.RELAY_URL,
        account: this.env.NEAR_ACCOUNT_ID,
        processed: this.processedEvents.size,
      });
    }

    if (request.method === "POST" && url.pathname === "/connect") {
      this.connect();
      return Response.json({ ok: true });
    }

    if (request.method === "POST" && url.pathname === "/disconnect") {
      this.disconnect();
      return Response.json({ ok: true });
    }

    return new Response("Not found", { status: 404 });
  }

  // ── Relay connection ───────────────────────────────────────────────

  private connect() {
    if (this.ws && this.ws.readyState === WebSocket.OPEN) return;

    console.log(`[watcher] connecting to ${this.env.RELAY_URL}`);
    this.ws = new WebSocket(this.env.RELAY_URL);

    this.ws.addEventListener("open", () => {
      console.log("[watcher] relay connected, subscribing");
      this.ws!.send(
        JSON.stringify([
          "REQ",
          "watcher-sub",
          { kinds: [1], "#t": [this.env.SUBSCRIPTION_TAG], limit: 100 },
        ]),
      );
    });

    this.ws.addEventListener("message", (event: MessageEvent) => {
      this.handleRelayMessage(event.data as string);
    });

    this.ws.addEventListener("close", () => {
      console.log("[watcher] relay disconnected");
      this.ws = null;
      this.scheduleReconnect();
    });

    this.ws.addEventListener("error", () => {
      console.log("[watcher] relay error");
    });
  }

  private disconnect() {
    if (this.reconnectTimer !== null) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
    if (this.ws) {
      this.ws.close(1000, "shutdown");
      this.ws = null;
    }
  }

  private scheduleReconnect() {
    if (this.reconnectTimer !== null) return;
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null;
      this.connect();
    }, 5000) as unknown as number;
  }

  // ── Relay message handling ──────────────────────────────────────────

  private handleRelayMessage(raw: string) {
    let msg: unknown;
    try { msg = JSON.parse(raw); } catch { return; }
    if (!Array.isArray(msg)) return;

    const [type] = msg as [string, ...unknown[]];

    if (type === "EVENT") {
      const [, , event] = msg as [string, string, NostrEvent];
      this.handleNostrEvent(event);
    } else if (type === "EOSE") {
      console.log("[watcher] caught up, listening for new events");
    }
  }

  private async handleNostrEvent(event: NostrEvent) {
    if (this.processedEvents.has(event.id)) return;
    this.processedEvents.add(event.id);

    if (this.processedEvents.size > 10000) {
      const arr = Array.from(this.processedEvents);
      this.processedEvents = new Set(arr.slice(-5000));
    }

    console.log(`[watcher] event ${event.id.slice(0, 12)} from ${event.pubkey.slice(0, 16)}`);
    console.log(`[watcher] content: ${event.content.slice(0, 120)}`);

    const action = this.parseAction(event);
    if (!action) {
      console.log(`[watcher] no valid action, skipping`);
      return;
    }

    const ok = await this.submitToNear(action, event);
    console.log(`[watcher] ${ok ? "OK" : "FAILED"} ${action.method} for ${event.id.slice(0, 12)}`);
  }

  // ── Parse action ──────────────────────────────────────────────────

  private parseAction(event: NostrEvent): { method: string; proposalId: number } | null {
    const content = event.content.toLowerCase();

    const hasTag = event.tags.some(
      (t) => t[0] === "t" && t[1]?.toLowerCase() === this.env.SUBSCRIPTION_TAG.toLowerCase(),
    );
    if (!hasTag) return null;

    const approve = content.match(/approve\s+proposal\s+(\d+)/);
    if (approve) return { method: "approve_with_event", proposalId: parseInt(approve[1]!, 10) };

    const cancel = content.match(/cancel_vote\s+proposal\s+(\d+)/);
    if (cancel) return { method: "cancel_vote_with_event", proposalId: parseInt(cancel[1]!, 10) };

    return null;
  }

  // ── Submit to NEAR ──────────────────────────────────────────────────

  private async submitToNear(action: { method: string; proposalId: number }, event: NostrEvent): Promise<boolean> {
    try {
      const txBody = await this.buildSignedTransaction(action.method, event);

      const res = await fetch(this.env.NEAR_RPC, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ jsonrpc: "2.0", id: Date.now(), method: "broadcast_tx_commit", params: [txBody] }),
      });

      const data = (await res.json()) as { error?: { message: string }; result?: { transaction_hash?: string; status?: { FinalExecutionStatus?: string } } };

      if (data.error) {
        console.log(`[watcher] NEAR error: ${data.error.message}`);
        return false;
      }

      console.log(`[watcher] tx ${data.result?.transaction_hash?.slice(0, 16)} = ${data.result?.status?.FinalExecutionStatus}`);
      return true;
    } catch (e: unknown) {
      console.log(`[watcher] submit error: ${e instanceof Error ? e.message : String(e)}`);
      return false;
    }
  }

  // ── Build signed NEAR transaction using Web Crypto ──────────────────

  private async buildSignedTransaction(methodName: string, event: NostrEvent): Promise<string> {
    const secretKeyB64 = this.env.NEAR_SIGNER_KEY.replace(/^ed25519:/, "");
    const secretKeyRaw = base64ToBytes(secretKeyB64);
    // near-api-js KeyPair.secretKey = base64(32-byte seed + 32-byte public)
    const seed = secretKeyRaw.slice(0, 32);
    const pubRaw = secretKeyRaw.slice(32, 64);
    const pubKeyStr = "ed25519:" + bytesToBase58(pubRaw);

    const cryptoKey = await crypto.subtle.importKey(
      "pkcs8",
      buildPkcs8Ed25519(seed),
      { name: "Ed25519", namedCurve: "Ed25519" },
      false,
      ["sign"],
    );

    const accountId = this.env.NEAR_ACCOUNT_ID;
    const contractId = this.env.TREASURY_CONTRACT_ID;

    // Build function call args
    const args = JSON.stringify({
      event: { id: event.id, pubkey: event.pubkey, created_at: event.created_at, kind: event.kind, tags: event.tags, content: event.content, sig: event.sig },
    });
    const argsB64 = btoa(args);

    // Fetch nonce and block hash
    const [nonceRes, blockRes] = await Promise.all([
      this.rpc("query", { request_type: "view_access_key", finality: "final", account_id: accountId, public_key: pubKeyStr }),
      this.rpc("block", { finality: "final" }),
    ]);

    const nonce = ((nonceRes as any)?.result?.nonce ?? 0) as number;
    const blockHash = ((blockRes as any)?.result?.header?.hash) as string;
    if (!blockHash) throw new Error("No block hash");

    // Serialize Transaction (borsh)
    const tx = serializeTransaction({
      signerId: accountId,
      publicKey: pubRaw,
      nonce: BigInt(nonce) + 1n,
      receiverId: contractId,
      actions: [{ type: "FunctionCall" as const, methodName, args: argsB64, gas: 30000000000000n, deposit: 0n }],
      blockHash: base58ToBytes(blockHash),
    });

    // Sign using Web Crypto
    const signature = new Uint8Array(await crypto.subtle.sign("Ed25519", cryptoKey, tx));

    // Serialize SignedTransaction
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
    if (!this.ws || this.ws.readyState !== WebSocket.OPEN) this.connect();
  }
}

// ── Worker entry ────────────────────────────────────────────────────

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const id = env.RELAY_WATCHER.idFromName("singleton");
    return env.RELAY_WATCHER.get(id).fetch(request);
  },
};

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
        u32(2), // FunctionCall variant
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
    u8(0), // PublicKey enum: ED25519
    tx.publicKey,
    u64(tx.nonce),
    borshStr(tx.receiverId),
    u32(actionBytes.length),
    ...actionBytes,
    tx.blockHash,
  ]);
}

function serializeSignedTx(signature: Uint8Array, tx: Uint8Array): Uint8Array {
  return concat([
    u8(0), // Signature keyType: ED25519
    signature,
    tx,
  ]);
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
  for (let i = 0; i < hex.length; i += 2) {
    bytes[pad + i / 2] = parseInt(hex.slice(i, i + 2), 16);
  }
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
  // RFC 8410: Ed25519 private key in PKCS#8
  // AlgorithmIdentifier: id-Ed25519 (1.3.101.112)
  // PrivateKey: OCTET STRING wrapping the 32-byte seed
  const algoId = new Uint8Array([0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70]);
  const version = new Uint8Array([0x02, 0x01, 0x00]); // INTEGER 0 (version)
  const privKeyWrap = new Uint8Array(4 + 32);
  privKeyWrap[0] = 0x04; // OCTET STRING tag
  privKeyWrap[1] = 32; // length
  privKeyWrap.set(seed, 2);
  const inner = concat([version, algoId, privKeyWrap]);
  const outer = new Uint8Array(2 + inner.length);
  outer[0] = 0x30; // SEQUENCE tag
  outer[1] = inner.length;
  outer.set(inner, 2);
  return outer;
}
