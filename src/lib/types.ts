/**
 * types.ts — Shared types
 */

export interface UserProfile {
  name?: string;
  display_name?: string;
  about?: string;
  picture?: string;
  banner?: string;
  website?: string;
  nip05?: string;
  lud16?: string;
}

export interface BindingValue {
  npub: string;
  relay: string;
  bound_at: number;
}

export interface BindingCache {
  bindings: Record<string, { npub: string; relay: string }>;
  pubkeyIndex: Record<string, string>; // npub → accountId
}

export interface MsigProposal {
  id: number;
  proposer: string;
  intent: string;
  params: Record<string, unknown>;
  approvals: string[];
  threshold: number;
  status: "active" | "approved" | "executed";
  created_at: number;
  expires_at: number | null;
}

export interface MsigWallet {
  account_id: string;
  signers: string[];
  threshold: number;
  near_balance: string;
}
