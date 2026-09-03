/**
 * near.ts — NEAR RPC via raw fetch. Matches clear-msig contract API.
 * All functions take contractId param (no hardcoded contract).
 */

import { NEAR_RPC } from "./constants";
import { buildGovEvent, extractEventFields, defaultExpiryNs } from "./schnorr";

async function rpcCall(method: string, params: Record<string, unknown> | unknown[]): Promise<any> {
  const res = await fetch(NEAR_RPC, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ jsonrpc: "2.0", id: 1, method, params }),
  });
  const json = await res.json();
  if (json.error) throw new Error(json.error.message);
  return json.result;
}

function decodeRpcResult(result: any): any {
  const raw = result?.result;
  if (!raw || !raw.length) return null;
  const bytes = Uint8Array.from(raw);
  const text = new TextDecoder().decode(bytes);
  let parsed: any;
  try {
    parsed = JSON.parse(text);
  } catch {
    // v2 contract wraps OBJECT returns without escaping the inner quotes:
    // {"result": "{"name":"stuffed",...}"} — invalid JSON, salvage the payload
    // between the `"result": "` opener and the final `"}` closer.
    const m = text.match(/^\s*\{\s*"result"\s*:\s*"(.*)"\s*\}\s*$/s);
    if (!m) return null;
    try { return JSON.parse(m[1]); } catch { return m[1]; }
  }
  // v2 lisp-rlm contract wraps every jsonReturnStr in { result: "..." }
  if (parsed && typeof parsed === "object" && !Array.isArray(parsed)
      && "result" in parsed && Object.keys(parsed).length === 1
      && (typeof parsed.result === "string" || typeof parsed.result === "number")) {
    return parsed.result;
  }
  return parsed;
}

export async function viewFunction(
  contractId: string,
  methodName: string,
  args: Record<string, unknown>,
): Promise<any> {
  const raw = await rpcCall("query", {
    request_type: "call_function",
    finality: "final",
    account_id: contractId,
    method_name: methodName,
    args_base64: btoa(JSON.stringify(args)),
  });
  return decodeRpcResult(raw);
}

// ── v2 nostr-gov contract bindings ─────────────────────────────────────
// Contract surface (contract-ts): init, create_wallet, pause, propose,
// approve, approve_with_event, execute, get_wallet, get_owner_nonce,
// is_paused, get_version, get_proposal, get_approvers,
// get_proposal_message, get_wallet_count, get_wallet_name,
// get_proposal_ids.

export interface Wallet {
  name: string;
  creator: string;
  created_at: string;
  deposit: string;
}

export interface Approvers {
  thr: string;
  pks: string;
}

export interface Proposal {
  id: string;
  wallet_name: string;
  st: "active" | "approved" | "executed";
  exp: string;
  amt: string;
  to: string;
  tk: string;
  act: "" | "appr" | "unp";
  np: string;
  nt: string;
  bl: string;
  ac: string;
}

function num(v: unknown): number {
  return Number(v ?? 0);
}

// v2 event-auth fields — every admin method requires `ev` (verifyOwnerEvent
// dies ERR_EV_REQUIRED when absent). `f` = extractEventFields(signed event).
export function eventAuthArgs(f: ReturnType<typeof extractEventFields>): Record<string, string> {
  return {
    pk: f.pubkey_hex,
    ev: f.event_id_hex,
    sig: f.sig_hex,
    kind: String(f.kind),
    tags: f.tags_json,
    ct: f.content,
    cat: String(f.created_at),
  };
}

// Sign a kind-37500 gov event (nsec locally, or via NIP-46 bunker) for an
// admin action string like `create_wallet:prod` or `propose:gov:3`.
export async function signGovEvent(
  contractId: string,
  action: string,
  signCtx: { secretKey: Uint8Array | null; signEventRaw: ((t: any) => Promise<any>) | null },
): Promise<Record<string, string>> {
  const nonce = await getOwnerNonce(contractId);
  const expiresAt = defaultExpiryNs();
  const { finalizeEvent } = await import("nostr-tools");
  const template = buildGovEvent({ action, nonce, expiresAt, contractId });
  const signed = signCtx.secretKey
    ? finalizeEvent(
        { kind: template.kind, content: template.content, tags: template.tags, created_at: template.created_at },
        signCtx.secretKey,
      )
    : await signCtx.signEventRaw!(template);
  return eventAuthArgs(extractEventFields(signed));
}

// Don't trust the wallet bridge: re-check the receipt on-chain and throw if
// ANY action in the tx failed (e.g. LackBalanceForState reverts silently in
// some bridges — the "treasury created" alert lied once because of this).
export async function verifyTxSuccess(txHash: string, signerId: string): Promise<void> {
  const res = await rpcCall("tx", [txHash, signerId]);
  const failures = (res?.receipts_outcome ?? [])
    .map((ro: any) => ro.outcome?.status)
    .filter((s: any) => s && "Failure" in s);
  if (failures.length > 0) {
    const kind = JSON.stringify(failures[0].Failure).slice(0, 200);
    throw new Error(`Tx failed on-chain: ${kind}`);
  }
}

// view raw string (jsonReturnStr results arrive JSON-encoded)
async function viewStr(contractId: string, method: string, args: Record<string, unknown>): Promise<string> {
  const r = await viewFunction(contractId, method, args);
  return typeof r === "string" ? r : String(r ?? "");
}

export async function getContractVersion(contractId: string): Promise<string> {
  return viewStr(contractId, "get_version", {});
}

export async function getOwnerNonce(contractId: string): Promise<number> {
  return num(await viewStr(contractId, "get_owner_nonce", {}));
}

export async function getEventNonce(contractId: string): Promise<number> {
  return getOwnerNonce(contractId);
}

export async function isPaused(contractId: string): Promise<boolean> {
  return (await viewStr(contractId, "is_paused", {})) === "1";
}

export async function getWalletCount(contractId: string): Promise<number> {
  return num(await viewStr(contractId, "get_wallet_count", {}));
}

export async function getWalletName(contractId: string, i: number): Promise<string> {
  const name = await viewStr(contractId, "get_wallet_name", { i: String(i) });
  // defensive: strip stray JSON quotes around registry names
  return name.replace(/^"|"$/g, "");
}

export async function listWallets(contractId: string, fromIndex = 0, limit = 50): Promise<string[]> {
  const total = await getWalletCount(contractId);
  const out: string[] = [];
  for (let i = fromIndex; i < Math.min(fromIndex + limit, total); i++) {
    const name = await getWalletName(contractId, i);
    if (name) out.push(name);
  }
  return out;
}

export async function getWallet(contractId: string, name: string): Promise<Wallet | null> {
  const raw = await viewFunction(contractId, "get_wallet", { name });
  if (!raw) return null;
  const w = typeof raw === "string" ? JSON.parse(raw) : raw;
  return w as Wallet;
}

export async function getApprovers(contractId: string, name: string): Promise<Approvers | null> {
  const raw = await viewFunction(contractId, "get_approvers", { name });
  if (!raw) return null;
  const a = typeof raw === "string" ? JSON.parse(raw) : raw;
  return a as Approvers;
}

// admin set = approvers of the implicit "gov" wallet; the v2 contract has
// no owner view, so until the gov wallet exists we surface the init owner
// (owner_npub0, readable only through this contract's legacy storage key —
// absent on fresh deploys, in which case admin actions stay hidden).
export async function getOwnerNpubs(contractId: string): Promise<string[]> {
  try {
    const a = await getApprovers(contractId, "gov");
    if (a?.pks) return a.pks.split(",");
  } catch { /* gov wallet not created yet */ }
  return [];
}

export async function getGuardianNpub(contractId: string): Promise<string | null> {
  const pks = await getOwnerNpubs(contractId);
  return pks[0] ?? null;
}

export async function getProposal(contractId: string, walletName: string, id: string): Promise<Proposal | null> {
  const raw = await viewFunction(contractId, "get_proposal", { name: walletName, id });
  if (!raw) return null;
  const p = typeof raw === "string" ? JSON.parse(raw) : raw;
  return { ...p, wallet_name: walletName } as Proposal;
}

export async function getProposalIds(contractId: string, walletName: string): Promise<string[]> {
  const joined = await viewStr(contractId, "get_proposal_ids", { name: walletName });
  return joined ? joined.split(",") : [];
}

export async function getProposalsPaginated(
  contractId: string,
  walletName: string,
  fromIndex = 0,
  limit = 20,
): Promise<Proposal[]> {
  const ids = await getProposalIds(contractId, walletName);
  const page = ids.slice(fromIndex, fromIndex + limit);
  const out: Proposal[] = [];
  for (const id of page) {
    const p = await getProposal(contractId, walletName, id);
    if (p) out.push(p);
  }
  return out;
}

export async function getWalletState(
  contractId: string,
  walletName: string,
): Promise<{ paused: boolean; approvers: Approvers | null; proposalIds: string[] } | null> {
  const [paused, approvers, proposalIds] = await Promise.all([
    isPaused(contractId),
    getApprovers(contractId, walletName),
    getProposalIds(contractId, walletName),
  ]);
  return { paused, approvers, proposalIds };
}

// per-wallet balance is not tracked as a view (deposits/payouts flow
// through the contract account); report the contract account balance
export async function getWalletNearBalance(contractId: string): Promise<string> {
  try {
    const r = await rpcCall("query", {
      request_type: "view_account",
      finality: "final",
      account_id: contractId,
    });
    return String(r?.amount ?? "0");
  } catch {
    return "0";
  }
}

// canonical approve message — the exact string the contract verifies
export async function getProposalMessage(
  contractId: string,
  walletName: string,
  id: string,
  approverIndex: number,
  expiresAtNs: string,
): Promise<string | null> {
  return viewStr(contractId, "get_proposal_message", {
    name: walletName,
    id,
    ix: String(approverIndex),
    exp: expiresAtNs,
  });
}

// spend stats need an indexer — not derivable from views alone
export async function getSpendStats(_contractId: string, _walletName: string): Promise<null> {
  return null;
}

// ── propose (admin, event-auth) ─────────────────────────────────────────
// The proposal id IS the event nonce; the action string must be
// `propose:<wallet>:<nonce>` to match verifyOwnerEvent on-chain.
export interface ProposeParams {
  walletName: string;
  expiresAt: string;      // proposal expiry (ns) — must be in the future
  action?: "" | "appr" | "unp"; // "" payout (default) · "appr" rotation · "unp" unpause (gov wallet only)
  amount?: string;        // payout: yocto NEAR
  recipient?: string;     // payout: account id
  token?: string;         // payout: NEP-141 contract (empty = native NEAR)
  newApprovers?: string;  // appr: comma-joined pubkeys
  newThreshold?: string;  // appr: threshold
}

export async function proposeProposal(
  contractId: string,
  params: ProposeParams,
  signCtx: { secretKey: Uint8Array | null; signEventRaw: ((t: any) => Promise<any>) | null },
): Promise<{ proposalId: string; args: Record<string, unknown> }> {
  const nonce = await getOwnerNonce(contractId);
  const act = params.action ?? (params.newApprovers ? "appr" : "");
  const action = `propose:${params.walletName}:${nonce}`;
  const { finalizeEvent } = await import("nostr-tools");
  const template = buildGovEvent({ action, nonce, expiresAt: params.expiresAt, contractId });
  const signed = signCtx.secretKey
    ? finalizeEvent(
        { kind: template.kind, content: template.content, tags: template.tags, created_at: template.created_at },
        signCtx.secretKey,
      )
    : await signCtx.signEventRaw!(template);
  const f = extractEventFields(signed);

  // NOTE: change-method calls go through the NEAR wallet (callMethod in the
  // page) — this function returns the args; the caller sends the tx.
  const args: Record<string, unknown> = {
    name: params.walletName,
    pexp: params.expiresAt,
    act,
    tk: act === "" ? (params.token ?? "") : "",
    ...eventAuthArgs(f),
  };
  if (act === "") {
    args.am = params.amount ?? "";
    args.rc = params.recipient ?? "";
  } else if (act === "appr") {
    args.np = params.newApprovers ?? "";
    args.nt = params.newThreshold ?? "1";
  }
  return { proposalId: String(nonce), args };
}

export async function getAllowedTokens(_contractId: string, _walletName: string): Promise<string[]> {
  return [];
}
