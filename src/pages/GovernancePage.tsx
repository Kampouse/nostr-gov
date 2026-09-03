/**
 * GovernancePage.tsx — Permissionless treasury creation + multisig governance
 * 
 * Creates <name>.<user-account.testnet> sub-accounts with clear-msig deployed.
 * Also supports viewing existing treasury contracts by contract ID.
 */

import { useState, useCallback } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Landmark, Wallet as WalletIcon, ChevronRight, Plus, Clock, Loader2, RefreshCw, Check, ExternalLink, Trash2, Shield, AlertTriangle, Send, Zap } from "lucide-react";
import { useAuth } from "../hooks/useAuth";
import { useNear } from "../hooks/useNearWallet";
import {
  viewFunction,
  getWallet, getWalletNearBalance, getWalletState,
  getOwnerNpubs, getGuardianNpub, getEventNonce, getContractVersion,
  getProposalsPaginated, getSpendStats,
  getProposalMessage, getOwnerNonce, listWallets,
  proposeProposal,
  type Wallet, type Proposal,
} from "../lib/near";
import { schnorrSign, defaultExpiryNs, buildOwnerMessage, buildApprovalEvent, buildGovEvent, extractEventFields } from "../lib/schnorr";
import { DEFAULT_TREASURY, NEAR_RPC, RELAYER_RELAYS } from "../lib/constants";
import { pool } from "../lib/nostr";
import type { Event } from "nostr-tools";
import { LoginScreen } from "../components/LoginScreen";

const STATUS_STYLES: Record<string, string> = {
  Active: "text-yellow bg-yellow/10 border-yellow/25",
  Approved: "text-neon bg-neon-dim border-neon/25",
  Executed: "text-text2 bg-surface2 border-brd",
  Cancelled: "text-red bg-red/10 border-red/25",
};

function timeAgo(ts: number): string {
  const s = Math.floor(Date.now() / 1000) - ts;
  if (s < 60) return `${s}s ago`;
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  if (s < 86400) return `${Math.floor(s / 3600)}h ago`;
  return `${Math.floor(s / 86400)}d ago`;
}

// LocalStorage for user's treasury list — the baked-in treasury always
// leads the list and can't be removed (removeTreasury is a no-op for it).
function getTreasuries(): string[] {
  let list: string[] = [];
  try { list = JSON.parse(localStorage.getItem("nostrgov-treasuries") || "[]"); }
  catch { list = []; }
  if (!list.includes(DEFAULT_TREASURY)) list.unshift(DEFAULT_TREASURY);
  return list;
}
function saveTreasury(id: string) {
  const list = getTreasuries();
  if (!list.includes(id)) { list.push(id); localStorage.setItem("nostrgov-treasuries", JSON.stringify(list)); }
}
function removeTreasury(id: string) {
  if (id === DEFAULT_TREASURY) return;
  localStorage.setItem("nostrgov-treasuries", JSON.stringify(getTreasuries().filter(t => t !== id)));
}

const DEPOSIT_NEAR = 3;
const STORAGE_DEPOSIT = "500000000000000000000000"; // 0.5 NEAR — matches contract's STORAGE_DEPOSIT_YOCTO

/** NEAR → yocto (integer math only, up to 24 decimals) */
function nearToYocto(v: string): string {
  const m = v.trim().match(/^(\d+)(?:\.(\d{1,24}))?$/);
  if (!m) return "";
  const frac = (m[2] ?? "").padEnd(24, "0");
  return (BigInt(m[1]) * 10n ** 24n + BigInt(frac || "0")).toString();
}

// ── Call a change method via the connected NEAR wallet ──
async function callMethod(
  wallet: any,
  contractId: string,
  methodName: string,
  args: Record<string, unknown>,
  opts?: { gas?: bigint; deposit?: bigint },
): Promise<any> {
  const tx = await wallet.signAndSendTransaction({
    receiverId: contractId,
    // HOT connector format ({type, params}) — the near-connect bridge
    // passes typed actions straight through; near-api-js class instances
    // hit its "Unsupported action type" parser throw.
    actions: [
      {
        type: "FunctionCall",
        params: {
          methodName,
          args,
          gas: String(opts?.gas ?? 300_000_000_000_000n),
          deposit: String(opts?.deposit ?? 0n),
        },
      },
    ],
  });
  return tx;
}

// ── Publish a signed event to relayer relays ──
async function publishToRelayerRelays(event: Event): Promise<void> {
  const results = await Promise.allSettled(
    RELAYER_RELAYS.map((r) => pool.publish([r], event)),
  );
  const ok = results.filter((r) => r.status === "fulfilled").length;
  console.log(`[relayer] published to ${ok}/${RELAYER_RELAYS.length} relays`);
  if (ok === 0) throw new Error("Failed to publish to any relay");
}

export default function GovernancePage() {
  const { pubkey, npub, secretKey, readOnly, signEventRaw } = useAuth();
  const { accountId, wallet } = useNear();
  const queryClient = useQueryClient();
  const [expanded, setExpanded] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const [error, setError] = useState("");
  const [treasuries, setTreasuries] = useState<string[]>(getTreasuries);
  const [newName, setNewName] = useState("");
  const [newWalletName, setNewWalletName] = useState("");
  const [creatingWallet, setCreatingWallet] = useState(false);
  const [walletError, setWalletError] = useState("");

  const createTreasury = useCallback(async () => {
    if (!accountId || !wallet || !pubkey || !newName.trim()) return;
    setCreating(true);
    setError("");
    try {
      const wasmRes = await fetch("/clear_msig.wasm");
      if (!wasmRes.ok) throw new Error("Failed to fetch WASM");
      const wasmBytes = new Uint8Array(await wasmRes.arrayBuffer());

      const treasuryId = `${newName.trim()}.${accountId}`;
      const depositYocto = (BigInt(DEPOSIT_NEAR) * 10n ** 24n).toString();
      const initArgs = { owner_npubs: [pubkey] };

      const tx = await wallet.signAndSendTransaction({
        receiverId: treasuryId,
        // HOT connector format — see callMethod comment
        actions: [
          { type: "CreateAccount" },
          { type: "Transfer", params: { deposit: depositYocto } },
          { type: "DeployContract", params: { code: wasmBytes } },
          { type: "FunctionCall", params: { methodName: "new", args: initArgs, gas: String(300_000_000_000_000n), deposit: "0" } },
        ],
      });

      saveTreasury(treasuryId);
      setTreasuries(getTreasuries());
      setNewName("");
      queryClient.invalidateQueries({ queryKey: ["treasury"] });
      alert(`Treasury created: ${treasuryId}\nTx: ${tx.transaction.hash}`);
    } catch (e: any) {
      setError(e.message || "Failed to create treasury");
    } finally {
      setCreating(false);
    }
  }, [accountId, wallet, pubkey, newName, queryClient]);

  const createWallet = useCallback(async (contractId: string) => {
    if (!wallet || !secretKey || !pubkey || !newWalletName.trim()) return;
    setCreatingWallet(true);
    setWalletError("");
    try {
      const expiresAt = defaultExpiryNs();
      const nonce = await getOwnerNonce(contractId);
      const action = `create_wallet:${newWalletName.trim()}`;
      const { finalizeEvent } = await import("nostr-tools");
      const signed = finalizeEvent(
        buildGovEvent({ action, nonce, expiresAt, contractId }),
        secretKey,
      );
      const f = extractEventFields(signed);

      await callMethod(wallet, contractId, "create_wallet", {
        name: newWalletName.trim(),
        // born self-custody: the signer is sole approver (rotate later
        // via an "appr" proposal)
        pks: pubkey,
        thr: "1",
        pk: f.pubkey_hex,
        sig: f.sig_hex,
        kind: String(f.kind),
        tags: f.tags_json,
        ct: f.content,
        cat: String(f.created_at),
      }, { deposit: BigInt(STORAGE_DEPOSIT) });

      setNewWalletName("");
      queryClient.invalidateQueries({ queryKey: ["treasury"] });
    } catch (e: any) {
      setWalletError(e.message || "Failed to create wallet");
    } finally {
      setCreatingWallet(false);
    }
  }, [wallet, secretKey, pubkey, newWalletName, queryClient]);

  if (!pubkey) return <LoginScreen />;

  const canSign = !!secretKey || !!signEventRaw;

  return (
    <div className="flex flex-col h-full overflow-y-auto">
      {/* Header */}
      <div className="flex items-center justify-between px-4 py-3 border-b border-brd shrink-0">
        <h1 className="text-[18px] font-bold">Governance</h1>
        <div className="flex items-center gap-1">
          <button
            onClick={() => queryClient.invalidateQueries({ queryKey: ["treasury"] })}
            className="p-2 rounded-[10px] text-text3 hover:text-text hover:bg-surface2 transition-colors"
          >
            <RefreshCw size={14} />
          </button>
        </div>
      </div>

      {/* Create treasury form */}
      {accountId && (
        <div className="px-4 py-3 border-b border-brd bg-surface shrink-0">
          <div className="text-[11px] text-text4 mb-2">Connected: {accountId}</div>
          <div className="flex gap-2">
            <input
              value={newName}
              onChange={(e) => setNewName(e.target.value.toLowerCase().replace(/[^a-z0-9_-]/g, ""))}
              placeholder="treasury-name"
              className="flex-1 min-w-0 px-3 py-2 rounded-[10px] bg-bg border border-brd text-text text-[13px] placeholder:text-text4 outline-none focus:border-neon/50"
              maxLength={32}
            />
            <button
              onClick={createTreasury}
              disabled={creating || newName.length < 2 || !pubkey}
              className="inline-flex items-center gap-1.5 px-4 py-2 rounded-[10px] text-[12px] font-semibold bg-neon text-bg border-none cursor-pointer hover:brightness-110 disabled:opacity-40 disabled:cursor-not-allowed whitespace-nowrap"
            >
              {creating ? <Loader2 size={12} className="animate-spin" /> : <Plus size={12} />}
              Create Treasury
            </button>
          </div>
          {pubkey && (
            <div className="text-[10px] text-text4 mt-1.5 truncate">
              Creates {newName || "name"}.{accountId} with your npub as owner · {DEPOSIT_NEAR} Ⓝ deposit
            </div>
          )}
          {error && <div className="text-red text-[11px] mt-1.5">{error}</div>}
        </div>
      )}

      {/* Treasury list */}
      <div className="p-2">
        {treasuries.map((contractId) => (
            <TreasuryCard
              key={contractId}
              contractId={contractId}
              isExpanded={expanded === contractId}
              onToggle={() => setExpanded(expanded === contractId ? null : contractId)}
              onRemove={contractId === DEFAULT_TREASURY ? null : () => { removeTreasury(contractId); setTreasuries(getTreasuries()); }}
              userNpub={pubkey}
              canSign={canSign}
              signEventRaw={signEventRaw}
              secretKey={secretKey}
              walletObj={wallet}
              onCreateWallet={async (name: string) => {
                const expiresAt = defaultExpiryNs();
                const nonce = await getOwnerNonce(contractId);
                const action = `create_wallet:${name}`;
                const { finalizeEvent } = await import("nostr-tools");
                const signed = finalizeEvent(
                  buildGovEvent({ action, nonce, expiresAt, contractId }),
                  secretKey!,
                );
                const f = extractEventFields(signed);
                return callMethod(wallet, contractId, "create_wallet", {
                  name, pks: pubkey!, thr: "1",
                  pk: f.pubkey_hex, sig: f.sig_hex, kind: String(f.kind),
                  tags: f.tags_json, ct: f.content, cat: String(f.created_at),
                }, { deposit: BigInt(STORAGE_DEPOSIT) });
              }}
            />
          ))}
      </div>
    </div>
  );
}

// ── Treasury Card ──

function TreasuryCard({
  contractId,
  isExpanded,
  onToggle,
  onRemove,
  userNpub,
  canSign,
  secretKey,
  walletObj,
  onCreateWallet,
  signEventRaw,
}: {
  contractId: string;
  isExpanded: boolean;
  onToggle: () => void;
  onRemove: (() => void) | null;
  userNpub: string;
  canSign: boolean;
  secretKey: Uint8Array | null;
  walletObj: any;
  signEventRaw: ((t: { kind: number; content: string; tags: string[][] }) => Promise<any>) | null;
  onCreateWallet: (name: string) => Promise<any>;
}) {
  const queryClient = useQueryClient();
  const [newWalletName, setNewWalletName] = useState("");
  const [creatingWallet, setCreatingWallet] = useState(false);
  const [walletError, setWalletError] = useState("");

  // Contract metadata
  const { data: meta, isLoading: loadingMeta } = useQuery({
    queryKey: ["treasury-meta", contractId],
    queryFn: async () => {
      const [version, ownerNpubs, walletCount] = await Promise.all([
        viewFunction(contractId, "get_version", {}),
        viewFunction(contractId, "get_owner_npubs", {}),
        viewFunction(contractId, "get_wallet_count", {}),
      ]);
      const wallets = await listWallets(contractId, 0, 50);
      return { version, ownerNpubs, walletCount, walletNames: wallets as string[] };
    },
    refetchInterval: 30_000,
  });

  const isOwner = Boolean(meta?.ownerNpubs?.includes(userNpub));
  const [expandedWallet, setExpandedWallet] = useState<string | null>(null);

  const handleCreateWallet = async () => {
    if (!newWalletName.trim()) return;
    setCreatingWallet(true);
    setWalletError("");
    try {
      await onCreateWallet(newWalletName.trim());
      setNewWalletName("");
      queryClient.invalidateQueries({ queryKey: ["treasury"] });
    } catch (e: any) {
      setWalletError(e.message || "Failed");
    } finally {
      setCreatingWallet(false);
    }
  };

  return (
    <div className="mb-1">
      <div className="flex items-center gap-1">
        <button
          onClick={onToggle}
          className="flex-1 flex items-center justify-between p-4 border border-brd rounded-[14px] bg-surface cursor-pointer transition-colors hover:border-neon/40 hover:bg-neon-dim/50"
        >
          <div className="flex items-center gap-3 text-left">
            <div className="w-9 h-9 rounded-[10px] bg-surface2 flex items-center justify-center text-text3">
              <Landmark size={16} />
            </div>
            <div>
              <div className="text-text text-[14px] font-semibold font-mono truncate max-w-[200px]">
                {contractId.split(".")[0]}
              </div>
              <div className="text-text4 text-[11px] mt-0.5 truncate max-w-[240px]">
                {contractId}{meta ? ` · v${String(meta.version)} · ${String(meta.walletCount)} wallet${Number(meta.walletCount) !== 1 ? "s" : ""}` : " · loading…"}
                {isOwner && " · "}<span className={isOwner ? "text-neon" : ""}>{isOwner ? "Owner" : "Viewer"}</span>
              </div>
            </div>
          </div>
          <ChevronRight size={16} className={`text-text4 transition-transform ${isExpanded ? "rotate-90" : ""}`} />
        </button>
        {onRemove && (
          <button
            onClick={(e) => { e.stopPropagation(); onRemove(); }}
            className="p-2 rounded-[10px] text-text4 hover:text-red hover:bg-red/10 transition-colors"
            title="Remove from list"
          >
            <Trash2 size={14} />
          </button>
        )}
      </div>

      {isExpanded && (
        <div className="px-4 pb-3" style={{ animation: "fade-up 0.2s ease-out" }}>
          {loadingMeta ? (
            <div className="flex items-center justify-center py-4 gap-2 text-text3 text-[13px]">
              <Loader2 size={14} className="animate-spin" /> Loading…
            </div>
          ) : (
            <>
              {/* Create wallet form */}
              {isOwner && canSign && (
                <div className="mb-3 p-3 border border-brd rounded-[12px] bg-surface">
                  <div className="text-[11px] text-text4 mb-2">Create multisig wallet (+0.5 Ⓝ storage)</div>
                  <div className="flex gap-2">
                    <input
                      value={newWalletName}
                      onChange={(e) => setNewWalletName(e.target.value.toLowerCase().replace(/[^a-z0-9_-]/g, ""))}
                      placeholder="wallet-name"
                      className="flex-1 min-w-0 px-3 py-1.5 rounded-[8px] bg-bg border border-brd text-text text-[13px] placeholder:text-text4 outline-none focus:border-neon/50"
                      maxLength={64}
                    />
                    <button
                      onClick={handleCreateWallet}
                      disabled={creatingWallet || newWalletName.length < 2}
                      className="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-[8px] text-[11px] font-semibold bg-neon text-bg border-none cursor-pointer hover:brightness-110 disabled:opacity-40"
                    >
                      {creatingWallet ? <Loader2 size={11} className="animate-spin" /> : <Plus size={11} />}
                      Create
                    </button>
                  </div>
                  {walletError && <div className="text-red text-[10px] mt-1.5">{walletError}</div>}
                </div>
              )}

              {/* Wallet list */}
              {(meta?.walletNames?.length ?? 0) === 0 ? (
                <p className="text-text4 text-[13px] text-center py-4">No wallets yet.</p>
              ) : (
                (meta?.walletNames ?? []).map((wName: string) => (
                  <WalletDetail
                    key={wName}
                    contractId={contractId}
                    walletName={wName}
                    isExpanded={expandedWallet === wName}
                    onToggle={() => setExpandedWallet(expandedWallet === wName ? null : wName)}
                    isOwner={isOwner}
                    canSign={canSign}
                    signEventRaw={signEventRaw}
                    userNpub={userNpub}
                    secretKey={secretKey}
                    walletObj={walletObj}
                  />
                ))
              )}
            </>
          )}
        </div>
      )}
    </div>
  );
}

// ── Wallet Detail ──

function WalletDetail({
  contractId,
  walletName,
  isExpanded,
  onToggle,
  isOwner,
  canSign,
  userNpub,
  secretKey,
  walletObj,
  signEventRaw,
}: {
  contractId: string;
  walletName: string;
  isExpanded: boolean;
  onToggle: () => void;
  isOwner: boolean;
  canSign: boolean;
  userNpub: string;
  secretKey: Uint8Array | null;
  walletObj: any;
  signEventRaw: ((t: { kind: number; content: string; tags: string[][] }) => Promise<any>) | null;
}) {
  const queryClient = useQueryClient();
  const [approvingId, setApprovingId] = useState<string | null>(null);
  const [executingId, setExecutingId] = useState<string | null>(null);
  const [actionError, setActionError] = useState("");
  const [useRelayer, setUseRelayer] = useState(false);
  const [relayerStatus, setRelayerStatus] = useState<"idle" | "publishing" | "submitted">("idle");
  const [showProposeForm, setShowProposeForm] = useState(false);
  const [proposingId, setProposingId] = useState<string | null>(null);
  const [payoutAmount, setPayoutAmount] = useState("");
  const [payoutTo, setPayoutTo] = useState("");
  const [newApproversInput, setNewApproversInput] = useState("");
  const [newThresholdInput, setNewThresholdInput] = useState("");
  const [newProposalId, setNewProposalId] = useState<string | null>(null);
  const [proposeMode, setProposeMode] = useState<"payout" | "people">("payout");

  const { data: balance } = useQuery({
    queryKey: ["wal-bal", contractId, walletName],
    queryFn: () => getWalletNearBalance(contractId),
    enabled: isExpanded,
  });

  const { data: state } = useQuery({
    queryKey: ["wal-state", contractId, walletName],
    queryFn: () => getWalletState(contractId, walletName),
    enabled: isExpanded,
  });

  const { data: proposals = [] } = useQuery({
    queryKey: ["wal-props", contractId, walletName],
    queryFn: () => getProposalsPaginated(contractId, walletName, 0, 20),
    enabled: isExpanded,
  });

  const { data: stats } = useQuery({
    queryKey: ["wal-stats", contractId, walletName],
    queryFn: () => getSpendStats(contractId, walletName),
    enabled: isExpanded,
  });

  const bal = balance ? (Number(balance) / 1e24).toFixed(3) : "—";
  const approvers = state?.approvers ?? null;
  const approverPks: string[] = approvers?.pks ? approvers.pks.split(",") : [];
  const threshold = Number(approvers?.thr ?? 1);

  // the user's index in the wallet's approver set
  function findApproverIndex(): number | null {
    const idx = approverPks.indexOf(userNpub);
    return idx >= 0 ? idx : null;
  }

  function approvalCount(p: Proposal): number {
    return Number(p.ac ?? 0);
  }

  function actLabel(act: string): string {
    if (act === "appr") return "Rotate approvers";
    if (act === "unp") return "Unpause";
    return "Payout";
  }

  const handleApprove = async (proposal: Proposal, cancel = false) => {
    if (!secretKey && !signEventRaw) return;

    const approverIndex = findApproverIndex();
    if (approverIndex === null) { setActionError("Your npub is not in the approver list"); return; }

    setApprovingId(proposal.id);
    setActionError("");
    setRelayerStatus("idle");
    try {
      const expiresAt = defaultExpiryNs();
      const message = await getProposalMessage(contractId, walletName, proposal.id, approverIndex, expiresAt);
      if (!message) throw new Error("Could not get proposal message");

      // v2 contract: relay path target (watcher submits approve_with_event);
      // direct path signs the message itself
      const methodName = "approve";

      // ── nsec path ──
      if (secretKey) {
        if (useRelayer) {
          // Sign kind-37500 locally, publish to relays for watcher to pick up
          const { finalizeEvent } = await import("nostr-tools");
          const evtTemplate = buildApprovalEvent({
            pubkey: userNpub,
            proposalMessage: message,
            contractId,
            walletName,
            proposalId: proposal.id,
            approverIndex,
            action: cancel ? "cancel" : "approve",
          });
          const signedEvent = finalizeEvent(
            { kind: evtTemplate.kind, content: evtTemplate.content, tags: evtTemplate.tags, created_at: evtTemplate.created_at },
            secretKey,
          );
          setRelayerStatus("publishing");
          await publishToRelayerRelays(signedEvent);
          setRelayerStatus("submitted");
          // Poll for on-chain change
          setTimeout(() => {
            queryClient.invalidateQueries({ queryKey: ["wal-props"] });
            queryClient.invalidateQueries({ queryKey: ["wal-state"] });
          }, 8000);
        } else {
          // Direct schnorr sign (cheaper, no watcher needed)
          const expiresAt = defaultExpiryNs();
          const signature = schnorrSign(message, secretKey);
          await callMethod(walletObj, contractId, "approve", {
            name: walletName,
            id: proposal.id,
            ix: String(approverIndex),
            pubkey_hex: userNpub,
            signature,
            expires_at: expiresAt,
          });
          queryClient.invalidateQueries({ queryKey: ["wal-props"] });
          queryClient.invalidateQueries({ queryKey: ["wal-state"] });
        }
        return;
      }

      // ── NIP-46 path ──
      if (signEventRaw) {
        if (useRelayer) {
          // Sign kind-37500 via bunker, publish to relays
          const evtTemplate = buildApprovalEvent({
            pubkey: userNpub,
            proposalMessage: message,
            contractId,
            walletName,
            proposalId: proposal.id,
            approverIndex,
            action: cancel ? "cancel" : "approve",
          });
          const signedEvent = await signEventRaw(evtTemplate);
          setRelayerStatus("publishing");
          await publishToRelayerRelays(signedEvent);
          setRelayerStatus("submitted");
          setTimeout(() => {
            queryClient.invalidateQueries({ queryKey: ["wal-props"] });
            queryClient.invalidateQueries({ queryKey: ["wal-state"] });
          }, 8000);
        } else {
          // Call contract directly via NEAR wallet
          const evtTemplate = buildApprovalEvent({
            pubkey: userNpub,
            proposalMessage: message,
            contractId,
            walletName,
            proposalId: proposal.id,
            approverIndex,
            action: "approve",
          });
          const signedEvent = await signEventRaw(evtTemplate);
          const fields = extractEventFields(signedEvent);
          await callMethod(walletObj, contractId, "approve_with_event", {
            pk: fields.pubkey_hex,
            sig: fields.sig_hex,
            kind: String(fields.kind),
            tags: fields.tags_json,
            ct: fields.content,
            cat: String(fields.created_at),
          });
          queryClient.invalidateQueries({ queryKey: ["wal-props"] });
          queryClient.invalidateQueries({ queryKey: ["wal-state"] });
        }
      }
    } catch (e: any) {
      setActionError(e.message || "Approval failed");
      setRelayerStatus("idle");
    } finally {
      setApprovingId(null);
    }
  };

  const handlePropose = async (p: { amount?: string; recipient?: string; newApprovers?: string; newThreshold?: string }) => {
    if ((!secretKey && !signEventRaw) || !walletObj) return;
    setProposingId("new");
    setActionError("");
    try {
      const expiresAt = defaultExpiryNs();
      const { proposalId, args } = await proposeProposal(contractId, {
        walletName,
        expiresAt,
        amount: p.amount,
        recipient: p.recipient,
        newApprovers: p.newApprovers,
        newThreshold: p.newThreshold,
      }, { secretKey, signEventRaw });
      await callMethod(walletObj, contractId, "propose", args);
      setNewProposalId(proposalId);
      setShowProposeForm(false);
      setPayoutAmount(""); setPayoutTo(""); setNewApproversInput(""); setNewThresholdInput("");
      queryClient.invalidateQueries({ queryKey: ["wal-props"] });
      queryClient.invalidateQueries({ queryKey: ["wal-state"] });
    } catch (e: any) {
      setActionError(e.message || "Propose failed");
    } finally {
      setProposingId(null);
    }
  };

  const handleExecute = async (proposal: Proposal) => {
    if (!secretKey || !walletObj) return;

    setExecutingId(proposal.id);
    setActionError("");
    try {
      const expiresAt = defaultExpiryNs();
      const nonce = await getOwnerNonce(contractId);
      const action = `execute:${walletName}:${proposal.id}`;
      const { finalizeEvent } = await import("nostr-tools");
      const template = buildGovEvent({
        action,
        nonce,
        expiresAt,
        contractId,
        content: "nostr-gov owner action",
      });
      const signed = finalizeEvent(
        { kind: template.kind, content: template.content, tags: template.tags, created_at: template.created_at },
        secretKey!,
      );
      const f = extractEventFields(signed);
      await callMethod(walletObj, contractId, "execute", {
        name: walletName,
        id: proposal.id,
        pk: f.pubkey_hex,
        sig: f.sig_hex,
        kind: String(f.kind),
        tags: f.tags_json,
        ct: f.content,
        cat: String(f.created_at),
      });

      queryClient.invalidateQueries({ queryKey: ["wal-props"] });
      queryClient.invalidateQueries({ queryKey: ["wal-state"] });
      queryClient.invalidateQueries({ queryKey: ["wal-bal"] });
    } catch (e: any) {
      setActionError(e.message || "Execution failed");
    } finally {
      setExecutingId(null);
    }
  };

  return (
    <div className="mt-2 border border-brd rounded-[12px] bg-surface overflow-hidden">
      <button
        onClick={onToggle}
        className="w-full flex items-center justify-between p-3 cursor-pointer hover:bg-surface2 transition-colors"
      >
        <div className="flex items-center gap-2.5">
          <WalletIcon size={14} className="text-text3" />
          <span className="text-text text-[13px] font-mono">{walletName}</span>
          <span className="text-text4 text-[11px]">{bal} Ⓝ · {proposals.length} proposals</span>
        </div>
        <ChevronRight size={14} className={`text-text4 transition-transform ${isExpanded ? "rotate-90" : ""}`} />
      </button>

      {isExpanded && (
        <div className="px-3 pb-3 border-t border-brd" style={{ animation: "fade-up 0.15s ease-out" }}>
          <div className="flex gap-2 mt-3">
            <div className="flex-1 p-2 rounded-[8px] bg-surface2 text-center">
              <div className="text-text4 text-[10px]">Proposals</div>
              <div className="text-text text-[12px] font-semibold mt-0.5">{proposals.length}</div>
            </div>
            <div className="flex-1 p-2 rounded-[8px] bg-surface2 text-center">
              <div className="text-text4 text-[10px]">Threshold</div>
              <div className="text-text text-[12px] font-semibold mt-0.5">{threshold} of {approverPks.length}</div>
            </div>
            <div className="flex-1 p-2 rounded-[8px] bg-surface2 text-center">
              <div className="text-text4 text-[10px]">Paused</div>
              <div className="text-text text-[12px] font-semibold mt-0.5">{state?.paused ? "yes" : "no"}</div>
            </div>
          </div>

          {/* Approver roster */}
          {approverPks.length > 0 && (
            <div className="mt-3 px-1">
              <div className="text-text4 text-[10px] mb-1">
                approvers · {threshold} of {approverPks.length} required
              </div>
              {approverPks.map((pk, i) => (
                <div key={`${pk}-${i}`} className="flex items-center gap-2 py-0.5">
                  <span className={`w-1.5 h-1.5 rounded-full shrink-0 ${userNpub === pk ? "bg-neon" : "bg-text4/40"}`} />
                  <span className="text-text3 text-[11px] font-mono truncate">{pk.slice(0, 16)}…{pk.slice(-8)}</span>
                  {userNpub === pk && <span className="text-neon text-[10px]">you</span>}
                  {isOwner && userNpub === pk && <span className="text-text4 text-[10px]">· can propose + execute</span>}
                </div>
              ))}
            </div>
          )}

          {!canSign && isOwner && (
            <div className="flex items-center gap-1.5 mt-3 px-2 py-2 rounded-[8px] bg-yellow/5 border border-yellow/20 text-yellow text-[11px]">
              <AlertTriangle size={12} />
              Connect with nsec or a NIP-46 bunker to approve proposals
            </div>
          )}
          {!isOwner && (
            <div className="text-text4 text-[10px] mt-3 px-2">
              Read-only — only the wallet's approvers can act here.
            </div>
          )}

          {canSign && isOwner && walletObj && (
            <div className="flex items-center justify-between mt-3 px-2 py-1.5">
              <button
                onClick={() => setShowProposeForm(!showProposeForm)}
                className={`flex items-center gap-1.5 text-[10px] px-2 py-1 rounded-[6px] border cursor-pointer transition-colors ${showProposeForm ? "text-neon border-neon/30 bg-neon/5" : "text-text4 border-brd hover:border-neon/20"}`}
              >
                <Plus size={10} />
                Propose
              </button>
              <div className="flex items-center gap-1">
                <button
                  onClick={() => setUseRelayer(!useRelayer)}
                  className={`flex items-center gap-1.5 text-[10px] px-2 py-1 rounded-[6px] border cursor-pointer transition-colors ${useRelayer ? "text-neon border-neon/30 bg-neon/5" : "text-text4 border-brd hover:border-neon/20"}`}
                >
                  <Zap size={10} />
                  Relayer
                </button>
                {useRelayer && relayerStatus === "submitted" && (
                  <span className="text-neon text-[10px]">Published, waiting for watcher…</span>
                )}
                {useRelayer && relayerStatus === "publishing" && (
                  <span className="text-yellow text-[10px] flex items-center gap-1"><Loader2 size={10} className="animate-spin" /> Publishing…</span>
                )}
              </div>
            </div>
          )}

          {canSign && isOwner && showProposeForm && (
            <div className="mt-2 p-3 border border-brd rounded-[10px] bg-bg">
              <div className="flex gap-1 mb-3">
                {(["payout", "people"] as const).map((m) => (
                  <button
                    key={m}
                    onClick={() => setProposeMode(m)}
                    className={`text-[10px] px-2 py-1 rounded-[6px] border cursor-pointer transition-colors ${proposeMode === m ? "text-neon border-neon/30 bg-neon/5" : "text-text4 border-brd hover:border-neon/20"}`}
                  >
                    {m === "payout" ? "Payout" : "People"}
                  </button>
                ))}
              </div>

              {proposeMode === "payout" ? (
                <div className="space-y-2">
                  <input
                    value={payoutAmount}
                    onChange={(e) => setPayoutAmount(e.target.value.replace(/[^0-9.]/g, ""))}
                    placeholder="amount in Ⓝ, e.g. 1.5"
                    className="w-full px-3 py-1.5 rounded-[8px] bg-surface border border-brd text-text text-[12px] placeholder:text-text4 outline-none focus:border-neon/50"
                  />
                  <input
                    value={payoutTo}
                    onChange={(e) => setPayoutTo(e.target.value.trim())}
                    placeholder="recipient.testnet"
                    className="w-full px-3 py-1.5 rounded-[8px] bg-surface border border-brd text-text text-[12px] placeholder:text-text4 outline-none focus:border-neon/50 font-mono"
                  />
                  <button
                    onClick={() => handlePropose({ amount: nearToYocto(payoutAmount), recipient: payoutTo })}
                    disabled={proposingId === "new" || !nearToYocto(payoutAmount) || !payoutTo.includes(".")}
                    className="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-[8px] text-[11px] font-semibold bg-neon text-bg border-none cursor-pointer hover:brightness-110 disabled:opacity-40"
                  >
                    {proposingId === "new" ? <Loader2 size={11} className="animate-spin" /> : <Plus size={11} />}
                    Propose payout
                  </button>
                </div>
              ) : (
                <div className="space-y-2">
                  <textarea
                    value={newApproversInput}
                    onChange={(e) => setNewApproversInput(e.target.value)}
                    placeholder={"approvers, one 64-hex npub per line"}
                    className="w-full px-3 py-1.5 rounded-[8px] bg-surface border border-brd text-text text-[12px] placeholder:text-text4 outline-none focus:border-neon/50 font-mono"
                    rows={3}
                  />
                  <input
                    value={newThresholdInput}
                    onChange={(e) => setNewThresholdInput(e.target.value.replace(/[^0-9]/g, ""))}
                    placeholder={`threshold, 1-${newApproversInput.split("\n").filter(s => s.trim()).length || 1}`}
                    className="w-full px-3 py-1.5 rounded-[8px] bg-surface border border-brd text-text text-[12px] placeholder:text-text4 outline-none focus:border-neon/50"
                  />
                  <button
                    onClick={() => handlePropose({
                      newApprovers: newApproversInput.split("\n").map(s => s.trim()).filter(Boolean).join(","),
                      newThreshold: newThresholdInput || "1",
                    })}
                    disabled={proposingId === "new" || newApproversInput.split("\n").filter(s => s.trim()).length === 0}
                    className="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-[8px] text-[11px] font-semibold bg-neon text-bg border-none cursor-pointer hover:brightness-110 disabled:opacity-40"
                  >
                    {proposingId === "new" ? <Loader2 size={11} className="animate-spin" /> : <Plus size={11} />}
                    Propose rotation
                  </button>
                  <div className="text-text4 text-[10px] mt-1">
                    Replaces this wallet's approver set with the listed npubs. Current approvers must approve it
                    (threshold {threshold} of {approverPks.length}).
                  </div>
                </div>
              )}
              {newProposalId && (
                <div className="text-neon text-[10px] mt-2">Proposal #{newProposalId} created — needs {threshold} approval{threshold !== 1 ? "s" : ""} before execute.</div>
              )}
            </div>
          )}

          {actionError && (
            <div className="mt-2 px-2 py-1.5 rounded-[8px] bg-red/10 border border-red/20 text-red text-[11px]">
              {actionError.length > 120 ? actionError.slice(0, 120) + "…" : actionError}
            </div>
          )}

          {proposals.length === 0 ? (
            <p className="text-text4 text-[12px] text-center py-4">No proposals yet.</p>
          ) : (
            proposals.map((p: Proposal) => {
              const threshold = Number(state?.approvers?.thr ?? 1);
              const approvals = approvalCount(p);
              const myApproverIdx = findApproverIndex();
              const alreadyApproved = myApproverIdx !== null && ((Number(p.bl ?? 0)) & (1 << myApproverIdx)) !== 0;
              const isApproved = p.st === "approved";
              const isExecuted = p.st === "executed";

              return (
                <div key={p.id} className="p-3 border border-brd rounded-[10px] bg-bg mt-2">
                  <div className="flex items-center justify-between">
                    <span className="text-text text-[12px] font-semibold">#{p.id} · {actLabel(p.act)}</span>
                    <span className={`text-[10px] font-semibold px-2 py-0.5 rounded-full border ${STATUS_STYLES[p.st] || "text-text3 bg-surface2 border-brd"}`}>
                      {p.st}
                    </span>
                  </div>
                  <div className="flex justify-between mt-1.5 text-text4 text-[10px]">
                    <span className="truncate max-w-[60%] font-mono">{p.to || "—"}</span>
                    <span className="flex items-center gap-1">
                      <Check size={10} /> {approvals}/{threshold}
                    </span>
                  </div>
                  <div className="flex justify-between mt-0.5 text-text4 text-[10px]">
                    <span className="flex items-center gap-1"><Clock size={10} /> {p.amt ? `${(Number(p.amt) / 1e24).toFixed(2)} Ⓝ` : ""}</span>
                    <span>expires {timeAgo(Number(p.exp) / 1_000_000_000)}</span>
                  </div>
                  {/* Action buttons */}
                  <div className="flex gap-1.5 mt-2">
                    {p.st === "active" && canSign && myApproverIdx !== null && !alreadyApproved && (
                      <button
                        onClick={() => handleApprove(p)}
                        disabled={approvingId === p.id}
                        className="inline-flex items-center gap-1 px-3 py-1.5 rounded-[8px] text-[11px] font-medium bg-neon text-bg border-none cursor-pointer hover:brightness-110 disabled:opacity-50"
                      >
                        {approvingId === p.id ? <Loader2 size={11} className="animate-spin" /> : <Shield size={11} />}
                        {useRelayer ? "Approve (relay)" : "Approve"}
                      </button>
                    )}
                    {alreadyApproved && !isApproved && !isExecuted && (
                      <span className="inline-flex items-center gap-1 px-2 py-1.5 text-[10px] text-neon">
                        <Check size={10} /> You approved
                      </span>
                    )}
                    {isApproved && !isExecuted && canSign && walletObj && (
                      <button
                        onClick={() => handleExecute(p)}
                        disabled={executingId === p.id}
                        className="inline-flex items-center gap-1 px-3 py-1.5 rounded-[8px] text-[11px] font-medium bg-surface2 text-text border border-brd cursor-pointer hover:border-neon/50 disabled:opacity-50"
                      >
                        {executingId === p.id ? <Loader2 size={11} className="animate-spin" /> : <Send size={11} />}
                        Execute
                      </button>
                    )}
                  </div>
                </div>
              );
            })
          )}
        </div>
      )}
    </div>
  );
}
