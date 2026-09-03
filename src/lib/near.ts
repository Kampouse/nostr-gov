/**
 * near.ts — NEAR RPC via raw fetch. Matches clear-msig contract API.
 * All functions take contractId param (no hardcoded contract).
 */

import { NEAR_RPC } from "./constants";

async function rpcCall(method: string, params: Record<string, unknown>): Promise<any> {
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
  return JSON.parse(text);
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
  return viewStr(contractId, "get_wallet_name", { i: String(i) });
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

// admin set = approvers of the implicit "gov" wallet
export async function getOwnerNpubs(contractId: string): Promise<string[]> {
  const a = await getApprovers(contractId, "gov");
  return a?.pks ? a.pks.split(",") : [];
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

export async function getAllowedTokens(_contractId: string, _walletName: string): Promise<string[]> {
  return [];
}
