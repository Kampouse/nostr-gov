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

// ── Contract read methods (all take contractId) ──

export interface Wallet {
  name: string;
  owner: string;
  proposal_index: number;
  intent_index: number;
  created_at: number;
}

export interface Proposal {
  id: number;
  wallet_name: string;
  intent_index: number;
  proposer: string;
  status: string;
  proposed_at: number;
  approved_at: number;
  expires_at: number;
  approval_bitmap: number;
  cancellation_bitmap: number;
  nostr_approval_bitmap: number;
  nostr_cancellation_bitmap: number;
  param_values: string;
  message: string;
  intent_params_hash: string;
}

export interface Intent {
  wallet_name: string;
  index: number;
  intent_type: string;
  name: string;
  template: string;
  proposers: string[];
  approvers: string[];
  nostr_approvers: string[];
  approval_threshold: number;
  cancellation_threshold: number;
  timelock_seconds: number;
  params: ParamDef[];
  execution_gas_tgas: number;
  active: boolean;
  active_proposal_count: number;
}

export interface ParamDef {
  name: string;
  param_type: string;
  max_value: string | null;
}

export async function getContractVersion(contractId: string): Promise<number> {
  const raw = await rpcCall("query", {
    request_type: "call_function",
    finality: "final",
    account_id: contractId,
    method_name: "get_version",
    args_base64: btoa("{}"),
  });
  const bytes = Uint8Array.from(raw.result as number[]);
  return parseInt(new TextDecoder().decode(bytes), 10);
}

export async function getWalletCount(contractId: string): Promise<number> {
  const raw = await rpcCall("query", {
    request_type: "call_function",
    finality: "final",
    account_id: contractId,
    method_name: "get_wallet_count",
    args_base64: btoa("{}"),
  });
  const bytes = Uint8Array.from(raw.result as number[]);
  return parseInt(new TextDecoder().decode(bytes), 10);
}

export async function getWallet(contractId: string, name: string): Promise<Wallet | null> {
  return viewFunction(contractId, "get_wallet", { name });
}

export async function getWalletState(contractId: string, name: string): Promise<any> {
  return viewFunction(contractId, "get_wallet_state", { wallet_name: name });
}

export async function getWalletNearBalance(contractId: string, name: string): Promise<string> {
  const raw = await rpcCall("query", {
    request_type: "call_function",
    finality: "final",
    account_id: contractId,
    method_name: "get_wallet_near_balance",
    args_base64: btoa(JSON.stringify({ wallet_name: name })),
  });
  const bytes = Uint8Array.from(raw.result as number[]);
  return new TextDecoder().decode(bytes);
}

export async function getOwnerNpubs(contractId: string): Promise<string[]> {
  return viewFunction(contractId, "get_owner_npubs", {});
}

export async function getProposal(contractId: string, walletName: string, id: number): Promise<Proposal | null> {
  return viewFunction(contractId, "get_proposal", { wallet_name: walletName, id });
}

export async function getProposalsPaginated(
  contractId: string,
  walletName: string,
  fromIndex: number = 0,
  limit: number = 20,
): Promise<Proposal[]> {
  return viewFunction(contractId, "get_proposals_paginated", {
    wallet_name: walletName,
    from_index: fromIndex,
    limit,
  });
}

export async function getSpendStats(contractId: string, walletName: string): Promise<any> {
  return viewFunction(contractId, "get_spend_stats", { wallet_name: walletName });
}

export async function getGuardianNpub(contractId: string): Promise<string | null> {
  return viewFunction(contractId, "get_guardian_npub", {});
}

export async function getEventNonce(contractId: string): Promise<number> {
  const raw = await rpcCall("query", {
    request_type: "call_function",
    finality: "final",
    account_id: contractId,
    method_name: "get_event_nonce",
    args_base64: btoa("{}"),
  });
  const bytes = Uint8Array.from(raw.result as number[]);
  return parseInt(new TextDecoder().decode(bytes), 10);
}

export async function getAllowedTokens(contractId: string, walletName: string): Promise<string[]> {
  return viewFunction(contractId, "get_allowed_tokens", { wallet_name: walletName });
}

export async function getProposalMessage(contractId: string, walletName: string, id: number): Promise<string | null> {
  return viewFunction(contractId, "get_proposal_message", { wallet_name: walletName, id });
}

export async function getOwnerNonce(contractId: string): Promise<number> {
  const raw = await rpcCall("query", {
    request_type: "call_function",
    finality: "final",
    account_id: contractId,
    method_name: "get_owner_nonce",
    args_base64: btoa("{}"),
  });
  const bytes = Uint8Array.from(raw.result as number[]);
  return parseInt(new TextDecoder().decode(bytes), 10);
}

export async function listWallets(contractId: string, fromIndex = 0, limit = 50): Promise<string[]> {
  return viewFunction(contractId, "list_wallets", { from_index: fromIndex, limit });
}
