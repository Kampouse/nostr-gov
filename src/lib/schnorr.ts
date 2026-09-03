import { schnorr } from "@noble/curves/secp256k1.js";
import { sha256 } from "@noble/hashes/sha2.js";

/** SHA-256 a string, return Uint8Array */
export const sha256Bytes = (msg: string): Uint8Array =>
  sha256(new TextEncoder().encode(msg));

/**
 * BIP-340 schnorr sign — matches the contract's verify_schnorr_signature.
 *
 * The contract does:
 *   msg_hash = SHA256(message.as_bytes())
 *   schnorr_verify(pubkey, sig, msg_hash)
 *
 * So we SHA256 the message first, then pass the hash to schnorr.sign.
 */
export const schnorrSign = (message: string, secretKey: Uint8Array): string => {
  const msgHash = sha256Bytes(message);
  const sig = schnorr.sign(msgHash, secretKey);
  return Array.from(sig).map(b => b.toString(16).padStart(2, "0")).join("");
};

/** Default expiry: now + 2 hours in nanoseconds */
export const defaultExpiryNs = (): string => {
  const secs = Math.floor(Date.now() / 1000) + 7200;
  return String(BigInt(secs) * BigInt(1000000000));
};

/**
 * Build an owner action message (for create_wallet, execute, propose).
 * Format: `expires {expires_at}.000000000: {action} | nonce: {nonce} | contract: {contract_id}`
 */
export const buildOwnerMessage = (
  action: string,
  nonce: number,
  expiresAt: string,
  contractId: string,
): string =>
  `expires ${expiresAt}.000000000: ${action} | nonce: ${nonce} | contract: ${contractId}`;

// ─── NIP-46 Event-Based Signing ───────────────────────────────────────────

/**
 * Build a nostr event for contract approval (NIP-46 compatible).
 * Returns the full event object ready to pass to an NDK signer.
 *
 * The contract verifies:
 *   1. Event ID = SHA256(JSON serialization)
 *   2. Schnorr sig over event ID (standard NIP-01)
 *   3. Event content matches the proposal message
 *   4. Pubkey matches the intent's nostr_approvers[index]
 */
export const buildApprovalEvent = (params: {
  pubkey: string; // npub hex
  proposalMessage: string;
  contractId: string;
  walletName: string;
  proposalId: string;
  approverIndex: number;
  action?: "approve" | "cancel";
}): NostrEventTemplate => {
  const { pubkey, proposalMessage, contractId, walletName, proposalId, approverIndex, action } = params;
  const tags: string[][] = [
    ["contract", contractId],
    ["wallet", walletName],
    ["proposal", String(proposalId)],
    ["approver", String(approverIndex)],
  ];
  if (action) tags.push(["action", action]);
  return {
    kind: 37500,
    content: proposalMessage,
    tags,
    created_at: Math.floor(Date.now() / 1000),
  };
};

/** Extract fields from a signed event for the contract call */
export const extractEventFields = (event: SignedNostrEvent) => ({
  pubkey_hex: event.pubkey,
  event_id_hex: event.id,
  created_at: event.created_at,
  kind: event.kind,
  tags_json: JSON.stringify(event.tags),
  content: event.content,
  sig_hex: event.sig,
});

// Types
export interface NostrEventTemplate {
  kind: number;
  content: string;
  tags: string[][];
  created_at: number;
}

export interface SignedNostrEvent extends NostrEventTemplate {
  id: string;
  pubkey: string;
  sig: string;
}
/** Gov (admin) event — v2 event-auth shape the contract verifies:
 * tags [t, action, nonce, expires, contract], kind 37500. */
export const buildGovEvent = (params: {
  action: string;
  nonce: number;
  expiresAt: string;
  contractId: string;
  content?: string;
}): NostrEventTemplate => ({
  kind: 37500,
  content: params.content ?? "nostr-gov owner action",
  tags: [
    ["t", "nostr-gov"],
    ["action", params.action],
    ["nonce", String(params.nonce)],
    ["expires", params.expiresAt],
    ["contract", params.contractId],
  ],
  created_at: Math.floor(Date.now() / 1000),
});

/** Gov envelope: kind-37500 event whose SIGNED content carries the full
 * contract args for propose/execute. The watcher whitelists methods (so
 * create_wallet can never ride the relayer) and submits args verbatim;
 * the contract re-verifies the admin schnorr sig over these exact bytes,
 * so any tampering with the payload dies on-chain (ERR_EVENT_SIG_INVALID).
 * Returns { event, args } — args are what the watcher will submit. */
export const buildGovEnvelope = async (params: {
  method: "propose" | "execute";
  contractId: string;
  walletName: string;
  proposalId?: string; // execute only
  expiresAt: string;
  args: Record<string, unknown>; // full contract args incl. amount/recipient
  signCtx: { secretKey: Uint8Array | null; signEventRaw: ((t: any) => Promise<any>) | null };
}): Promise<{ event: any; args: Record<string, unknown> }> => {
  const nonce = await import("./near").then((m) => m.getOwnerNonce(params.contractId));
  const action = params.method === "execute"
    ? `execute:${params.walletName}:${params.proposalId}`
    : `propose:${params.walletName}:${nonce}`;
  const payload = { ...params.args };
  const template = buildGovEvent({
    action,
    nonce,
    expiresAt: params.expiresAt,
    contractId: params.contractId,
    content: "gov:" + JSON.stringify({
      v: 1,
      method: params.method,
      contractId: params.contractId,
      args: payload,
    }),
  });
  const { finalizeEvent } = await import("nostr-tools");
  const signed = params.signCtx.secretKey
    ? finalizeEvent(
        { kind: template.kind, content: template.content, tags: template.tags, created_at: template.created_at },
        params.signCtx.secretKey,
      )
    : await params.signCtx.signEventRaw!(template);
  return { event: signed, args: payload };
};
