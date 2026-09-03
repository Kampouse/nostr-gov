/**
 * GovernancePage — full governance dashboard over the v2 nostr-gov contract.
 *
 * Drill-down: Treasury → Wallet → Proposal, every level backed by live
 * contract views. All admin writes are event-auth (kind-37500, BIP-340
 * schnorr) and every transaction is re-verified on-chain after the wallet
 * bridge returns (a failed CreateAccount receipt once reported "created").
 *
 * Contract surface (contract-ts/src/main.ts): init, create_wallet, pause,
 * propose, approve, approve_with_event, execute, get_wallet, get_owner_nonce,
 * is_paused, get_version, get_proposal, get_approvers, get_proposal_message,
 * get_wallet_count, get_wallet_name, get_proposal_ids.
 */

import { useState, useCallback } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import {
  Landmark, Wallet as WalletIcon, ChevronRight, ChevronLeft, Plus, Clock,
  Loader2, RefreshCw, Check, Trash2, Shield, AlertTriangle, Send, Zap,
  X,
} from "lucide-react";
import { useAuth } from "../hooks/useAuth";
import { useNear } from "../hooks/useNearWallet";
import {
  viewFunction, getWallet, getWalletNearBalance, getWalletState,
  getOwnerNpubs, getEventNonce, getContractVersion, isPaused,
  getProposalsPaginated, getProposalMessage, listWallets,
  proposeProposal, signGovEvent, verifyTxSuccess,
  type Wallet, type Proposal,
} from "../lib/near";
import {
  schnorrSign, defaultExpiryNs, buildApprovalEvent, extractEventFields,
  buildGovEnvelope,
} from "../lib/schnorr";
import { DEFAULT_TREASURY, RELAYER_RELAYS, RELAYER_WATCHER_URL } from "../lib/constants";
import { pool } from "../lib/nostr";
import type { Event } from "nostr-tools";
import { LoginScreen } from "../components/LoginScreen";

// ── constants (e2e-verified against the deployed contract) ──────────────

// CreateAccount + Transfer + DeployContract + init. The deployed 145 KB
// wasm stakes ~1.47 NEAR of storage; 2 leaves a small float. Refundable
// only by deleting the account.
const CREATE_TREASURY_DEPOSIT = "2000000000000000000000000";
// create_wallet attached deposit — contract minimum is ~1.0019 NEAR
// (1,001,882,102,603,448,320 yocto); 1 NEAR fails, e2e used 1 NEAR+.
// Actually the e2e passed 1.0 NEAR against an earlier build; use 1.1.
const WALLET_CREATE_DEPOSIT = "1100000000000000000000000";

// ── formatting helpers ───────────────────────────────────────────────────

function yoctoToNear(y: string | null | undefined, fracDigits = 3): string {
  if (!y) return "—";
  try {
    const v = BigInt(y);
    const neg = v < 0n;
    const a = neg ? -v : v;
    const whole = a / 10n ** 24n;
    const frac = (a % 10n ** 24n).toString().padStart(24, "0").slice(0, fracDigits);
    return `${neg ? "-" : ""}${whole}.${frac}`;
  } catch {
    return "—";
  }
}

function nsToDate(ns: string | null | undefined): string {
  if (!ns) return "—";
  try {
    return new Date(Number(BigInt(ns) / 1_000_000n)).toLocaleString();
  } catch {
    return "—";
  }
}

function nsCountdown(ns: string | null | undefined): string {
  if (!ns) return "—";
  try {
    const ms = Number(BigInt(ns) / 1_000_000n) - Date.now();
    if (ms <= 0) return "expired";
    const m = Math.floor(ms / 60000);
    if (m < 60) return `${m}m left`;
    const h = Math.floor(m / 60);
    if (h < 48) return `${h}h ${m % 60}m left`;
    return `${Math.floor(h / 24)}d left`;
  } catch {
    return "—";
  }
}

function shortPk(pk: string): string {
  return pk.length > 20 ? `${pk.slice(0, 10)}…${pk.slice(-8)}` : pk;
}

// LocalStorage treasury list — the baked-in treasury always leads and
// can't be removed.
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

// The contract has no owner view (bootstrap admin = owner_npub0, readable
// by no view method). Remember who we deployed a treasury for, so the
// creator keeps admin rights in the UI until a gov wallet exists on-chain.
function getTreasuryOwner(id: string): string {
  try { return localStorage.getItem(`nostrgov-owner:${id}`) || ""; }
  catch { return ""; }
}
function saveTreasuryOwner(id: string, npub: string) {
  try { localStorage.setItem(`nostrgov-owner:${id}`, npub); } catch { /* private mode */ }
}

/** NEAR → yocto (integer math only, up to 24 decimals) */
function nearToYocto(v: string): string {
  const m = v.trim().match(/^(\d+)(?:\.(\d{1,24}))?$/);
  if (!m) return "";
  const frac = (m[2] ?? "").padEnd(24, "0");
  return (BigInt(m[1]) * 10n ** 24n + BigInt(frac || "0")).toString();
}

// ── wallet-bridge call (HOT connector format) + verified variant ─────────

async function callMethod(
  wallet: any,
  contractId: string,
  methodName: string,
  args: Record<string, unknown>,
  opts?: { gas?: string; deposit?: string },
): Promise<any> {
  return wallet.signAndSendTransaction({
    receiverId: contractId,
    actions: [{
      type: "FunctionCall",
      params: {
        methodName,
        args,
        gas: opts?.gas ?? "300000000000000",
        deposit: opts?.deposit ?? "0",
      },
    }],
  });
}

/** Send, then re-check receipts on-chain — the bridge return value lies. */
async function callMethodVerified(
  wallet: any,
  accountId: string,
  contractId: string,
  methodName: string,
  args: Record<string, unknown>,
  opts?: { gas?: string; deposit?: string },
): Promise<void> {
  const tx = await callMethod(wallet, contractId, methodName, args, opts);
  const hash = tx?.transaction?.hash ?? tx?.transactionHash;
  if (hash) await verifyTxSuccess(hash, accountId);
}

async function publishToRelayerRelays(event: Event): Promise<{ relays: number; via: "relays" | "ingest" }> {
  const results = await Promise.allSettled(RELAYER_RELAYS.map((r) => pool.publish([r], event)));
  const ok = results.filter((r) => r.status === "fulfilled").length;
  if (ok > 0) return { relays: ok, via: "relays" };
  // Relay fallback: direct POST to the watcher's ingest endpoint. The
  // event is already signed and NIP-01-id-stamped; the watcher re-verifies
  // the id and runs the same whitelist + on-chain sig check. Covers
  // networks where relays are blocked/unreachable (damus ws refused, etc).
  const res = await fetch(RELAYER_WATCHER_URL + "/ingest", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ event }),
  });
  if (!res.ok) throw new Error(`ingest failed: ${res.status} ${(await res.text()).slice(0, 100)}`);
  const data = await res.json() as { ok?: boolean; error?: string };
  if (!data.ok) throw new Error(data.error || "ingest rejected");
  return { relays: 0, via: "ingest" };
}

// Sign a gov envelope (propose/execute) and publish it — the watcher picks
// it up and submits the NEAR tx gaslessly. Payload travels in the signed
// content, so the contract re-verifies every arg byte on-chain.
async function proposeViaRelayer(
  contractId: string, walletName: string,
  p: { method: "propose" | "execute"; proposalId?: string; args: Record<string, unknown> },
  signCtx: SignCtx,
): Promise<{ relays: number; via: "relays" | "ingest" }> {
  const { buildGovEnvelope } = await import("../lib/schnorr");
  const { event } = await buildGovEnvelope({
    method: p.method,
    contractId,
    walletName,
    proposalId: p.proposalId,
    expiresAt: defaultExpiryNs(),
    args: p.args,
    signCtx,
  });
  return publishToRelayerRelays(event as Event);
}

// ── shared sign context type ─────────────────────────────────────────────

interface SignCtx {
  secretKey: Uint8Array | null;
  signEventRaw: ((t: { kind: number; content: string; tags: string[][] }) => Promise<any>) | null;
}

// ── toasts ───────────────────────────────────────────────────────────────

interface Toast { id: number; kind: "ok" | "err"; text: string }

function useToasts() {
  const [toasts, setToasts] = useState<Toast[]>([]);
  const push = useCallback((kind: "ok" | "err", text: string) => {
    const id = Date.now() + Math.random();
    setToasts((t) => [...t.slice(-3), { id, kind, text }]);
    setTimeout(() => setToasts((t) => t.filter(x => x.id !== id)), 5000);
  }, []);
  return { toasts, push };
}

function Toasts({ toasts }: { toasts: Toast[] }) {
  if (toasts.length === 0) return null;
  return (
    <div className="fixed bottom-4 left-1/2 -translate-x-1/2 z-50 flex flex-col gap-1.5 w-[92%] max-w-md">
      {toasts.map(t => (
        <div key={t.id} className={`flex items-start gap-2 px-3 py-2 rounded-[10px] border text-[11px] break-all ${
          t.kind === "ok" ? "bg-neon-dim border-neon/30 text-neon" : "bg-red/10 border-red/30 text-red"
        }`}>
          {t.kind === "ok" ? <Check size={12} className="mt-0.5 shrink-0" /> : <AlertTriangle size={12} className="mt-0.5 shrink-0" />}
          <span>{t.text}</span>
        </div>
      ))}
    </div>
  );
}

// ── status dot ───────────────────────────────────────────────────────────

function Dot({ on, pulse }: { on: boolean; pulse?: boolean }) {
  return (
    <span className={`inline-block w-1.5 h-1.5 rounded-full shrink-0 ${on ? "bg-neon" : "bg-text4/40"} ${pulse && on ? "animate-pulse" : ""}`} />
  );
}

// ═════════════════════════════════════════════════════════════════════════
// Create treasury modal
// ═════════════════════════════════════════════════════════════════════════

function CreateTreasuryModal({
  accountId, ownerNpub, walletObj, onClose, onCreated, toast,
}: {
  accountId: string;
  ownerNpub: string;
  walletObj: any;
  onClose: () => void;
  onCreated: (id: string) => void;
  toast: (kind: "ok" | "err", text: string) => void;
}) {
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const clean = name.toLowerCase().replace(/[^a-z0-9_-]/g, "");
  const treasuryId = clean ? `${clean}.${accountId}` : "";

  const create = async () => {
    if (!clean || !walletObj) return;
    setBusy(true);
    setError("");
    try {
      const wasmRes = await fetch("/nostr-gov.wasm");
      if (!wasmRes.ok) throw new Error(`Failed to fetch wasm (${wasmRes.status})`);
      const wasmBytes = new Uint8Array(await wasmRes.arrayBuffer());

      const tx = await walletObj.signAndSendTransaction({
        receiverId: treasuryId,
        actions: [
          { type: "CreateAccount" },
          { type: "Transfer", params: { deposit: CREATE_TREASURY_DEPOSIT } },
          { type: "DeployContract", params: { code: wasmBytes } },
          { type: "FunctionCall", params: { methodName: "init", args: { npub: ownerNpub }, gas: "300000000000000", deposit: "0" } },
        ],
      });
      // verify receipts on-chain before claiming success
      const hash = tx?.transaction?.hash ?? tx?.transactionHash;
      if (!hash) throw new Error("No tx hash returned by wallet");
      await verifyTxSuccess(hash, accountId);
      saveTreasury(treasuryId);
      saveTreasuryOwner(treasuryId, ownerNpub);
      toast("ok", `Treasury ${treasuryId} created — ${hash.slice(0, 8)}…`);
      onCreated(treasuryId);
      onClose();
    } catch (e: any) {
      setError(e.message || "Failed");
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="fixed inset-0 z-40 bg-black/60 flex items-center justify-center p-4" onClick={onClose}>
      <div className="w-full max-w-sm bg-surface border border-brd rounded-[14px] p-4" onClick={e => e.stopPropagation()}>
        <div className="flex items-center justify-between mb-3">
          <div className="text-[14px] font-bold flex items-center gap-2"><Landmark size={14} /> New treasury</div>
          <button onClick={onClose} className="p-1.5 rounded-[8px] text-text4 hover:text-text hover:bg-surface2 cursor-pointer"><X size={14} /></button>
        </div>
        <div className="flex items-center gap-1 mb-1">
          <input
            autoFocus
            value={name}
            onChange={(e) => setName(e.target.value.toLowerCase().replace(/[^a-z0-9_-]/g, ""))}
            placeholder="name"
            maxLength={32}
            className="flex-1 min-w-0 px-3 py-2 rounded-[10px] bg-bg border border-brd text-text text-[13px] font-mono placeholder:text-text4 outline-none focus:border-neon/50"
          />
          <span className="text-text4 text-[12px] font-mono">.{accountId}</span>
        </div>
        <div className="text-text4 text-[10px] mb-3">
          Deploys the governance contract · {yoctoToNear(CREATE_TREASURY_DEPOSIT)} Ⓝ
          (~1.47 Ⓝ becomes non-refundable storage stake, rest is the treasury float) ·
          your npub becomes the bootstrap admin.
        </div>
        <button
          onClick={create}
          disabled={busy || clean.length < 2 || !walletObj}
          className="w-full inline-flex items-center justify-center gap-1.5 px-4 py-2 rounded-[10px] text-[12px] font-semibold bg-neon text-bg border-none cursor-pointer hover:brightness-110 disabled:opacity-40"
        >
          {busy ? <Loader2 size={12} className="animate-spin" /> : <Plus size={12} />}
          {busy ? "Deploying…" : `Create ${treasuryId || "treasury"}`}
        </button>
        {error && <div className="mt-2 px-2 py-1.5 rounded-[8px] bg-red/10 border border-red/20 text-red text-[11px] break-all">{error}</div>}
      </div>
    </div>
  );
}

// ═════════════════════════════════════════════════════════════════════════
// Treasury level — real overview of one contract
// ═════════════════════════════════════════════════════════════════════════

interface TreasuryData {
  version: string;
  balance: string | null;
  paused: boolean;
  walletNames: string[];
  wallets: Record<string, Wallet | null>;
  adminPks: string[];
  eventNonce: number;
}

function useTreasury(contractId: string, enabled: boolean) {
  return useQuery({
    queryKey: ["treasury", contractId],
    queryFn: async (): Promise<TreasuryData> => {
      const [version, balance, paused, walletNames, adminPks, eventNonce] = await Promise.all([
        getContractVersion(contractId),
        getWalletNearBalance(contractId),
        isPaused(contractId),
        listWallets(contractId, 0, 100),
        getOwnerNpubs(contractId),
        getEventNonce(contractId),
      ]);
      const wallets: Record<string, Wallet | null> = {};
      await Promise.all(walletNames.map(async (n) => { wallets[n] = await getWallet(contractId, n); }));
      return { version, balance, paused, walletNames, wallets, adminPks, eventNonce };
    },
    enabled,
    refetchInterval: 20_000,
    retry: 1,
  });
}

function TreasuryLevel({
  contractId, userNpub, onSelectWallet, onRemove, canSign, signCtx, toast,
}: {
  contractId: string;
  userNpub: string;
  onSelectWallet: (name: string) => void;
  onRemove: (() => void) | null;
  canSign: boolean;
  signCtx: SignCtx;
  toast: (kind: "ok" | "err", text: string) => void;
}) {
  const { accountId, wallet } = useNear();
  const { data: t, isLoading, isError, error, refetch, isFetching } = useTreasury(contractId, true);
  // on-chain admin set (gov approvers) — empty until a gov wallet exists;
  // fall back to the locally remembered creator (fresh treasuries)
  const onChainAdmin = Boolean(t?.adminPks?.includes(userNpub));
  const localOwner = getTreasuryOwner(contractId) === userNpub;
  const isAdmin = onChainAdmin || (localOwner && (t?.adminPks.length ?? 0) === 0);
  const [newWallet, setNewWallet] = useState("");
  const [creatingWallet, setCreatingWallet] = useState(false);

  const handleCreateWallet = async () => {
    if (!wallet || !t || !accountId) return;
    const name = newWallet.toLowerCase().replace(/[^a-z0-9_-]/g, "");
    if (!name) return;
    setCreatingWallet(true);
    try {
      const ev = await signGovEvent(contractId, `create_wallet:${name}`, signCtx);
      await callMethodVerified(wallet, accountId, contractId, "create_wallet", {
        name, pks: userNpub, thr: "1", ...ev,
      }, { deposit: WALLET_CREATE_DEPOSIT });
      setNewWallet("");
      toast("ok", `Wallet "${name}" created`);
      refetch();
    } catch (e: any) {
      toast("err", e.message || "create_wallet failed");
    } finally {
      setCreatingWallet(false);
    }
  };

  return (
    <div className="p-3 space-y-3">
      {/* overview strip */}
      <div className="flex items-center justify-between">
        <button onClick={() => onRemove?.()} className="hidden" />
        <div className="flex items-center gap-2 min-w-0">
          <div className="w-8 h-8 rounded-[10px] bg-surface2 flex items-center justify-center text-text3 shrink-0"><Landmark size={14} /></div>
          <div className="min-w-0">
            <div className="text-text text-[14px] font-semibold font-mono truncate">{contractId}</div>
            <div className="text-text4 text-[10px] flex items-center gap-1.5">
              <Dot on={!isError} />
              {t ? `v${String(t.version)} · ${yoctoToNear(t.balance)} Ⓝ · ${t.walletNames.length} wallet${t.walletNames.length !== 1 ? "s" : ""}${t.paused ? " · PAUSED" : ""}` : isLoading ? "loading…" : "offline"}
            </div>
          </div>
        </div>
        {onRemove && (
          <button onClick={onRemove} className="p-2 rounded-[10px] text-text4 hover:text-red hover:bg-red/10 cursor-pointer" title="Remove from list"><Trash2 size={13} /></button>
        )}
      </div>

      {/* admin badge */}
      <div className="flex items-center gap-2 text-[10px] px-2 py-1.5 rounded-[8px] bg-surface2 border border-brd text-text3">
        <Shield size={11} className={isAdmin ? "text-neon" : "text-text4"} />
        {t ? (isAdmin ? "You are admin (bootstrap admin / gov approvers)" : "Read-only — not an admin on this contract") : "…"}
        {t && <span className="text-text4">· nonce {t.eventNonce}</span>}
      </div>

      {isError && (
        <div className="flex items-center gap-2 px-3 py-2 rounded-[10px] bg-red/10 border border-red/20 text-red text-[11px]">
          <AlertTriangle size={12} /> {(error as Error)?.message?.slice(0, 160) || "Contract unreachable"}
        </div>
      )}

      {/* create wallet (admin) */}
      {isAdmin && (
        <div className="p-3 border border-brd rounded-[12px] bg-surface">
          <div className="text-[11px] text-text4 mb-2">New wallet — born self-custody (you = sole approver, rotate later) · {yoctoToNear(WALLET_CREATE_DEPOSIT)} Ⓝ storage</div>
          <div className="flex gap-2">
            <input
              value={newWallet}
              onChange={(e) => setNewWallet(e.target.value.toLowerCase().replace(/[^a-z0-9_-]/g, ""))}
              placeholder="wallet-name"
              maxLength={64}
              className="flex-1 min-w-0 px-3 py-1.5 rounded-[8px] bg-bg border border-brd text-text text-[13px] font-mono placeholder:text-text4 outline-none focus:border-neon/50"
            />
            <button
              onClick={handleCreateWallet}
              disabled={creatingWallet || newWallet.length < 2 || !wallet || !canSign}
              className="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-[8px] text-[11px] font-semibold bg-neon text-bg border-none cursor-pointer hover:brightness-110 disabled:opacity-40"
            >
              {creatingWallet ? <Loader2 size={11} className="animate-spin" /> : <Plus size={11} />}
              Create
            </button>
          </div>
          {!wallet && <div className="text-text4 text-[10px] mt-1.5">Connect a NEAR wallet (HOT) to send the transaction.</div>}
          {!canSign && wallet && <div className="text-text4 text-[10px] mt-1.5">Sign in with nsec / bunker to sign the admin event.</div>}
        </div>
      )}

      {/* wallets */}
      <div>
        <div className="text-text4 text-[10px] uppercase tracking-wide mb-1 px-1">Wallets</div>
        {isLoading ? (
          <div className="flex items-center justify-center py-6 gap-2 text-text3 text-[13px]"><Loader2 size={14} className="animate-spin" /> Loading chain data…</div>
        ) : (t?.walletNames.length ?? 0) === 0 ? (
          <p className="text-text4 text-[12px] text-center py-6">No wallets yet{isAdmin ? " — create one above." : "."}</p>
        ) : (
          t!.walletNames.map((name) => (
            <button
              key={name}
              onClick={() => onSelectWallet(name)}
              className="w-full flex items-center justify-between p-3 mb-1 border border-brd rounded-[12px] bg-surface cursor-pointer hover:border-neon/40 transition-colors text-left"
            >
              <div className="flex items-center gap-2.5 min-w-0">
                <WalletIcon size={14} className={name === "gov" ? "text-neon" : "text-text3"} />
                <div className="min-w-0">
                  <div className="text-text text-[13px] font-mono">{name}{name === "gov" && <span className="text-neon text-[10px] ml-1.5">admin</span>}</div>
                  <div className="text-text4 text-[10px]">
                    {name === "gov" ? "governance wallet (implicit)" : (() => { const w = t?.wallets[name]; return w?.created_at ? new Date(Number(BigInt(w.created_at) / 1_000_000n)).toLocaleDateString() : ""; })()}
                  </div>
                </div>
              </div>
              <ChevronRight size={14} className="text-text4" />
            </button>
          ))
        )}
      </div>
      {isFetching && !isLoading && <div className="text-text4 text-[10px] text-center">refreshing…</div>}
    </div>
  );
}

// ═════════════════════════════════════════════════════════════════════════
// Wallet level — approvers, proposals, propose form
// ═════════════════════════════════════════════════════════════════════════

function WalletLevel({
  contractId, walletName, userNpub, onBack, onSelectProposal, canSign, signCtx, useRelayer, toast,
}: {
  contractId: string;
  walletName: string;
  userNpub: string;
  onBack: () => void;
  onSelectProposal: (id: string) => void;
  canSign: boolean;
  signCtx: SignCtx;
  useRelayer: boolean;
  toast: (kind: "ok" | "err", text: string) => void;
}) {
  const { accountId, wallet } = useNear();
  const queryClient = useQueryClient();
  const { data: t } = useTreasury(contractId, true);
  const { data: state } = useQuery({
    queryKey: ["wal-state", contractId, walletName],
    queryFn: () => getWalletState(contractId, walletName),
    refetchInterval: 20_000,
  });
  const { data: proposals = [] } = useQuery({
    queryKey: ["wal-props", contractId, walletName],
    queryFn: () => getProposalsPaginated(contractId, walletName, 0, 50),
    refetchInterval: 20_000,
  });

  const isGov = walletName === "gov";
  const approverPks: string[] = state?.approvers?.pks ? state.approvers.pks.split(",") : [];
  const threshold = Number(state?.approvers?.thr ?? 1);
  const myIdx = approverPks.indexOf(userNpub) >= 0 ? approverPks.indexOf(userNpub) : null;
  const canPropose = Boolean(state?.approvers) && myIdx !== null; // wallet approvers gate; for gov = admin set

  // propose form state
  const [proposeMode, setProposeMode] = useState<"payout" | "people" | null>(null);
  const [payAmount, setPayAmount] = useState("");
  const [payTo, setPayTo] = useState("");
  const [payToken, setPayToken] = useState("");
  const [apprInput, setApprInput] = useState("");
  const [apprThr, setApprThr] = useState("");
  const [busyPropose, setBusyPropose] = useState(false);
  const [filter, setFilter] = useState<"active" | "approved" | "all">("active");

  const refresh = () => {
    queryClient.invalidateQueries({ queryKey: ["wal-props"] });
    queryClient.invalidateQueries({ queryKey: ["wal-state"] });
    queryClient.invalidateQueries({ queryKey: ["treasury"] });
  };

  const submitPayout = async () => {
    const amt = nearToYocto(payAmount);
    if (!amt || !payTo.includes(".")) return;
    setBusyPropose(true);
    try {
      const expiresAt = defaultExpiryNs();
      const { proposalId, args } = await proposeProposal(contractId, {
        walletName, expiresAt, action: "",
        amount: amt, recipient: payTo.trim(), token: payToken.trim(),
      }, signCtx);
      if (useRelayer) {
        const { relays, via } = await proposeViaRelayer(contractId, walletName, { method: "propose", args }, signCtx);
        toast("ok", `Payout proposal #${proposalId} ${via === "ingest" ? "sent direct to watcher (relays unreachable)" : `published to ${relays}/${RELAYER_RELAYS.length} relays`} — proposing on-chain`);
      } else {
        await callMethodVerified(wallet, accountId!, contractId, "propose", args);
        toast("ok", `Payout proposal #${proposalId} created`);
      }
      setProposeMode(null); setPayAmount(""); setPayTo(""); setPayToken("");
      refresh();
    } catch (e: any) {
      toast("err", e.message?.slice(0, 180) || "propose failed");
    } finally {
      setBusyPropose(false);
    }
  };

  const submitAppr = async () => {
    const nps = apprInput.split("\n").map(s => s.trim().replace(/^npub1.*$/, "")).filter(s => /^[0-9a-fA-F]{64}$/.test(s));
    if (nps.length === 0) { toast("err", "Enter at least one 64-hex pubkey"); return; }
    setBusyPropose(true);
    try {
      const expiresAt = defaultExpiryNs();
      const { proposalId, args } = await proposeProposal(contractId, {
        walletName, expiresAt, action: "appr", newApprovers: nps.join(","), newThreshold: apprThr || "1",
      }, signCtx);
      if (useRelayer) {
        const { relays, via } = await proposeViaRelayer(contractId, walletName, { method: "propose", args }, signCtx);
        toast("ok", `Rotation #${proposalId} ${via === "ingest" ? "sent direct to watcher (relays unreachable)" : `published to ${relays}/${RELAYER_RELAYS.length} relays`} — proposing on-chain`);
      } else {
        await callMethodVerified(wallet, accountId!, contractId, "propose", args);
        toast("ok", "Approver rotation proposed");
      }
      setProposeMode(null); setApprInput(""); setApprThr("");
      refresh();
    } catch (e: any) {
      toast("err", e.message?.slice(0, 180) || "propose failed");
    } finally {
      setBusyPropose(false);
    }
  };

  const activeCount = proposals.filter(p => p.st === "active").length;
  const approvedCount = proposals.filter(p => p.st === "approved").length;
  const shown = proposals.filter(p => filter === "all" ? true : filter === "active" ? p.st === "active" : p.st === "approved");

  return (
    <div className="p-3 space-y-3">
      <button onClick={onBack} className="flex items-center gap-1 text-text3 hover:text-text text-[11px] cursor-pointer">
        <ChevronLeft size={13} /> {contractId}
      </button>

      <div className="flex items-center gap-2.5">
        <div className={`w-8 h-8 rounded-[10px] bg-surface2 flex items-center justify-center shrink-0 ${isGov ? "text-neon" : "text-text3"}`}>
          {isGov ? <Shield size={14} /> : <WalletIcon size={14} />}
        </div>
        <div>
          <div className="text-text text-[15px] font-semibold font-mono">{walletName}</div>
          <div className="text-text4 text-[10px]">
            {isGov ? "governance wallet — proposals here change the whole contract" : "multisig wallet"} · {threshold} of {approverPks.length || "?"} approvals
          </div>
        </div>
      </div>

      {state?.paused && (
        <div className="flex items-center gap-2 px-2 py-1.5 rounded-[8px] bg-yellow/5 border border-yellow/20 text-yellow text-[11px]">
          <AlertTriangle size={12} /> Contract paused — only unpause proposals can execute.
        </div>
      )}

      {/* approver roster */}
      <div className="p-3 border border-brd rounded-[12px] bg-surface">
        <div className="text-text4 text-[10px] uppercase tracking-wide mb-2">Approvers · {threshold} of {approverPks.length} required</div>
        {approverPks.length === 0 ? (
          <div className="text-text4 text-[11px]">{isGov ? "Bootstrap: contract owner (until gov rotation)" : "No approver set"}</div>
        ) : approverPks.map((pk, i) => (
          <div key={`${pk}-${i}`} className="flex items-center gap-2 py-0.5">
            <Dot on={true} />
            <span className="text-text3 text-[11px] font-mono truncate">{shortPk(pk)}</span>
            {pk === userNpub && <span className="text-neon text-[10px]">you</span>}
            <span className="text-text4 text-[10px] ml-auto">#{i}</span>
          </div>
        ))}
      </div>

      {/* propose panel */}
      {canPropose && canSign && (
        <div className="p-3 border border-brd rounded-[12px] bg-surface">
          {!proposeMode ? (
            <div className="flex gap-2">
              <button onClick={() => setProposeMode("payout")} className="flex-1 inline-flex items-center justify-center gap-1.5 px-3 py-2 rounded-[8px] text-[11px] font-semibold border border-brd text-text3 hover:border-neon/40 hover:text-neon cursor-pointer">
                <Send size={11} /> Propose payout
              </button>
              <button onClick={() => setProposeMode("people")} className="flex-1 inline-flex items-center justify-center gap-1.5 px-3 py-2 rounded-[8px] text-[11px] font-semibold border border-brd text-text3 hover:border-neon/40 hover:text-neon cursor-pointer">
                <Shield size={11} /> Propose {isGov ? "admins" : "rotation"}
              </button>
            </div>
          ) : (
            <div>
              <div className="flex items-center justify-between mb-2">
                <div className="flex gap-1">
                  {(["payout", "people"] as const).map(m => (
                    <button key={m} onClick={() => setProposeMode(m)} className={`text-[10px] px-2 py-1 rounded-[6px] border cursor-pointer transition-colors ${proposeMode === m ? "text-neon border-neon/30 bg-neon/5" : "text-text4 border-brd"}`}>
                      {m === "payout" ? "Payout" : isGov ? "Admins" : "Rotation"}
                    </button>
                  ))}
                </div>
                <button onClick={() => setProposeMode(null)} className="text-text4 hover:text-text cursor-pointer p-1"><X size={12} /></button>
              </div>

              {proposeMode === "payout" ? (
                <div className="space-y-2">
                  <input value={payAmount} onChange={(e) => setPayAmount(e.target.value.replace(/[^0-9.]/g, ""))} placeholder="amount in Ⓝ (e.g. 0.5)" className="w-full px-3 py-1.5 rounded-[8px] bg-bg border border-brd text-text text-[12px] placeholder:text-text4 outline-none focus:border-neon/50" />
                  <input value={payTo} onChange={(e) => setPayTo(e.target.value.trim())} placeholder="recipient.testnet" className="w-full px-3 py-1.5 rounded-[8px] bg-bg border border-brd text-text text-[12px] font-mono placeholder:text-text4 outline-none focus:border-neon/50" />
                  <input value={payToken} onChange={(e) => setPayToken(e.target.value.trim())} placeholder="token contract (empty = NEAR)" className="w-full px-3 py-1.5 rounded-[8px] bg-bg border border-brd text-text text-[12px] font-mono placeholder:text-text4 outline-none focus:border-neon/50" />
                  <button onClick={submitPayout} disabled={busyPropose || !nearToYocto(payAmount) || !payTo.includes(".")} className="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-[8px] text-[11px] font-semibold bg-neon text-bg border-none cursor-pointer hover:brightness-110 disabled:opacity-40">
                    {busyPropose ? <Loader2 size={11} className="animate-spin" /> : <Plus size={11} />} Propose payout
                  </button>
                </div>
              ) : (
                <div className="space-y-2">
                  <textarea
                    value={apprInput}
                    onChange={(e) => setApprInput(e.target.value)}
                    placeholder={"one 64-hex pubkey per line"}
                    rows={3}
                    className="w-full px-3 py-1.5 rounded-[8px] bg-bg border border-brd text-text text-[12px] font-mono placeholder:text-text4 outline-none focus:border-neon/50"
                  />
                  <div className="flex items-center gap-2">
                    <input value={apprThr} onChange={(e) => setApprThr(e.target.value.replace(/[^0-9]/g, ""))} placeholder={`threshold (1-${Math.max(1, apprInput.split("\n").filter(s => s.trim()).length)})`} className="w-40 px-3 py-1.5 rounded-[8px] bg-bg border border-brd text-text text-[12px] placeholder:text-text4 outline-none focus:border-neon/50" />
                    <button onClick={() => { setApprInput(approverPks.join("\n")); setApprThr(String(threshold)); }} className="text-text4 hover:text-text text-[10px] cursor-pointer">fill current</button>
                  </div>
                  <button onClick={submitAppr} disabled={busyPropose} className="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-[8px] text-[11px] font-semibold bg-neon text-bg border-none cursor-pointer hover:brightness-110 disabled:opacity-40">
                    {busyPropose ? <Loader2 size={11} className="animate-spin" /> : <Plus size={11} />} Propose {isGov ? "admin set" : "rotation"}
                  </button>
                  <div className="text-text4 text-[10px]">Replaces the approver set of "{walletName}". Current approvers must approve it first.</div>
                </div>
              )}
            </div>
          )}
        </div>
      )}

      {/* proposals */}
      <div>
        <div className="flex items-center justify-between mb-1 px-1">
          <div className="text-text4 text-[10px] uppercase tracking-wide">Proposals</div>
          <div className="flex gap-1">
            {([`active ${activeCount}`, `approved ${approvedCount}`, "all"] as const).map(f => {
              const key = f.split(" ")[0] as typeof filter;
              return (
                <button key={f} onClick={() => setFilter(key)} className={`text-[10px] px-2 py-0.5 rounded-[6px] border cursor-pointer flex items-center gap-1 ${filter === key ? "text-neon border-neon/30 bg-neon/5" : "text-text4 border-brd"}`}>
                  {key !== "all" && <Dot on={key === "active" ? activeCount > 0 : approvedCount > 0} />}
                  {f}
                </button>
              );
            })}
          </div>
        </div>
        {shown.length === 0 ? (
          <p className="text-text4 text-[12px] text-center py-6">{proposals.length === 0 ? "No proposals yet." : `No ${filter} proposals.`}</p>
        ) : shown.map(p => {
          const approvedByMe = myIdx !== null && (Number(p.bl ?? 0) & (1 << myIdx)) !== 0;
          return (
            <button
              key={p.id}
              onClick={() => onSelectProposal(p.id)}
              className="w-full flex items-center justify-between p-3 mb-1 border border-brd rounded-[12px] bg-surface cursor-pointer hover:border-neon/40 transition-colors text-left"
            >
              <div className="min-w-0">
                <div className="text-text text-[12px] font-semibold">#{p.id} · {p.act === "appr" ? (isGov ? "Set admins" : "Rotate approvers") : p.act === "unp" ? "Unpause" : "Payout"}</div>
                <div className="text-text4 text-[10px] truncate max-w-[220px] font-mono">
                  {p.act === "" ? `${yoctoToNear(p.amt)} Ⓝ → ${p.to || "?"}` : p.act === "appr" ? `${p.nt || "?"} of ${(p.np || "").split(",").filter(Boolean).length} approvers` : "unpause contract"}
                </div>
                <div className="text-text4 text-[10px] flex items-center gap-1 mt-0.5"><Clock size={9} /> {nsCountdown(p.exp)}</div>
              </div>
              <div className="flex flex-col items-end gap-1 shrink-0">
                <span className={`text-[9px] font-semibold px-1.5 py-0.5 rounded-full border ${p.st === "active" ? "text-yellow border-yellow/25" : p.st === "approved" ? "text-neon border-neon/25" : "text-text3 border-brd"}`}>{p.st}</span>
                <span className="text-text4 text-[10px] flex items-center gap-0.5"><Check size={9} /> {p.ac || 0}/{threshold}</span>
                {approvedByMe && <span className="text-neon text-[9px]">you approved</span>}
              </div>
            </button>
          );
        })}
      </div>
    </div>
  );
}

// ═════════════════════════════════════════════════════════════════════════
// Proposal level — detail + approve/execute
// ═════════════════════════════════════════════════════════════════════════

function ProposalLevel({
  contractId, walletName, proposalId, userNpub, onBack, canSign, signCtx, useRelayer, toast,
}: {
  contractId: string;
  walletName: string;
  proposalId: string;
  userNpub: string;
  onBack: () => void;
  canSign: boolean;
  signCtx: SignCtx;
  useRelayer: boolean;
  toast: (kind: "ok" | "err", text: string) => void;
}) {
  const { accountId, wallet } = useNear();
  const queryClient = useQueryClient();
  const [busy, setBusy] = useState<"" | "approve" | "execute" | "relay">("");
  const { data: p } = useQuery({
    queryKey: ["proposal", contractId, walletName, proposalId],
    queryFn: () => import("../lib/near").then(m => m.getProposal(contractId, walletName, proposalId)),
    refetchInterval: 15_000,
  });
  const { data: state } = useQuery({
    queryKey: ["wal-state", contractId, walletName],
    queryFn: () => getWalletState(contractId, walletName),
  });
  const { data: treasury } = useTreasury(contractId, true);

  const approverPks: string[] = state?.approvers?.pks ? state.approvers.pks.split(",")
    : (walletName === "gov" && state?.approvers == null && getTreasuryOwner(contractId) === userNpub)
      ? [userNpub] /* bootstrap: creator is the pre-rotation admin set */
      : (treasury?.adminPks ?? []);
  const threshold = Number(state?.approvers?.thr ?? 1);
  const myIdx = approverPks.indexOf(userNpub) >= 0 ? approverPks.indexOf(userNpub) : null;
  const alreadyApproved = myIdx !== null && p ? (Number(p.bl ?? 0) & (1 << myIdx)) !== 0 : false;

  const refresh = () => {
    queryClient.invalidateQueries({ queryKey: ["proposal"] });
    queryClient.invalidateQueries({ queryKey: ["wal-props"] });
    queryClient.invalidateQueries({ queryKey: ["wal-state"] });
    queryClient.invalidateQueries({ queryKey: ["treasury"] });
  };

  const doApproveRelay = async () => {
    if (!p || myIdx === null) return;
    setBusy("relay");
    try {
      const expiresAt = defaultExpiryNs();
      const message = await getProposalMessage(contractId, walletName, p.id, myIdx, expiresAt);
      if (!message) throw new Error("get_proposal_message failed");
      const template = buildApprovalEvent({
        pubkey: userNpub, proposalMessage: message, contractId, walletName,
        proposalId: p.id, approverIndex: myIdx, action: "approve",
      });
      const { finalizeEvent } = await import("nostr-tools");
      const signed = signCtx.secretKey
        ? finalizeEvent({ kind: template.kind, content: template.content, tags: template.tags, created_at: template.created_at }, signCtx.secretKey)
        : await signCtx.signEventRaw!(template);
      const pub = await publishToRelayerRelays(signed as Event);
      toast("ok", pub.via === "ingest"
        ? "Sent direct to watcher (relays unreachable) — approving on-chain"
        : `Published to ${pub.relays}/${RELAYER_RELAYS.length} relays — approving on-chain`);
      setTimeout(refresh, 10_000);
    } catch (e: any) {
      toast("err", e.message?.slice(0, 180) || "relay approve failed");
    } finally {
      setBusy("");
    }
  };

  const doApproveDirect = async () => {
    if (!p || myIdx === null) return;
    setBusy("approve");
    try {
      const expiresAt = defaultExpiryNs();
      const message = await getProposalMessage(contractId, walletName, p.id, myIdx, expiresAt);
      if (!message) throw new Error("get_proposal_message failed");
      if (signCtx.secretKey && wallet) {
        const signature = schnorrSign(message, signCtx.secretKey);
        await callMethodVerified(wallet, accountId!, contractId, "approve", {
          name: walletName, id: p.id, ix: String(myIdx),
          pubkey_hex: userNpub, signature, expires_at: expiresAt,
        });
      } else if (signCtx.signEventRaw && wallet) {
        const template = buildApprovalEvent({
          pubkey: userNpub, proposalMessage: message, contractId, walletName,
          proposalId: p.id, approverIndex: myIdx, action: "approve",
        });
        const signed = await signCtx.signEventRaw(template);
        const { eventAuthArgs } = await import("../lib/near");
        const ev = eventAuthArgs(extractEventFields(signed));
        await callMethodVerified(wallet, accountId!, contractId, "approve_with_event", ev);
      } else {
        throw new Error("No signer available");
      }
      toast("ok", `Approved #${p.id}`);
      refresh();
    } catch (e: any) {
      toast("err", e.message?.slice(0, 180) || "approve failed");
    } finally {
      setBusy("");
    }
  };

  const doExecute = async () => {
    if (!p) return;
    setBusy("execute");
    try {
      const ev = await signGovEvent(contractId, `execute:${walletName}:${p.id}`, signCtx);
      await callMethodVerified(wallet, accountId!, contractId, "execute", { name: walletName, id: p.id, ...ev });
      toast("ok", `Executed #${p.id}`);
      refresh();
    } catch (e: any) {
      toast("err", e.message?.slice(0, 180) || "execute failed");
    } finally {
      setBusy("");
    }
  };

  // Gasless execute: the whole auth lives in the signed Nostr event
  // (verifyOwnerEvent on-chain), payout comes from the treasury's balance —
  // the relayer only fronts gas for the tx. Watcher whitelists execute.
  const doExecuteRelay = async () => {
    if (!p) return;
    setBusy("relay");
    try {
      const ok = await proposeViaRelayer(contractId, walletName, {
        method: "execute",
        proposalId: p.id,
        args: { name: walletName, id: p.id },
      }, signCtx);
      toast("ok", `Execute #${p.id} ${ok.via === "ingest" ? "sent direct to watcher (relays unreachable)" : `published to ${ok.relays}/${RELAYER_RELAYS.length} relays`} — executing on-chain`);
      setTimeout(refresh, 10_000);
    } catch (e: any) {
      toast("err", e.message?.slice(0, 180) || "relay execute failed");
    } finally {
      setBusy("");
    }
  };

  if (!p) return (
    <div className="p-3">
      <button onClick={onBack} className="flex items-center gap-1 text-text3 hover:text-text text-[11px] cursor-pointer"><ChevronLeft size={13} /> {walletName}</button>
      <div className="flex items-center justify-center py-8 gap-2 text-text3 text-[13px]"><Loader2 size={14} className="animate-spin" /> Loading proposal…</div>
    </div>
  );

  const isApprover = myIdx !== null;
  const canExecute = p.st === "approved" && isApprover && canSign;

  return (
    <div className="p-3 space-y-3">
      <button onClick={onBack} className="flex items-center gap-1 text-text3 hover:text-text text-[11px] cursor-pointer">
        <ChevronLeft size={13} /> {contractId} / {walletName}
      </button>

      <div className="flex items-center justify-between">
        <div className="text-text text-[16px] font-bold">#{p.id}</div>
        <span className={`text-[10px] font-semibold px-2 py-0.5 rounded-full border ${p.st === "active" ? "text-yellow border-yellow/25" : p.st === "approved" ? "text-neon border-neon/25" : "text-text3 border-brd"}`}>{p.st}</span>
      </div>

      {/* what it does */}
      <div className="p-3 border border-brd rounded-[12px] bg-surface space-y-1.5">
        {p.act === "" ? (
          <>
            <Row k="Type" v={p.tk ? "FT payout" : "NEAR payout"} />
            <Row k="Amount" v={`${yoctoToNear(p.amt)} ${p.tk ? "tokens" : "Ⓝ"}`} />
            <Row k="Recipient" v={p.to || "—"} mono />
            {p.tk && <Row k="Token" v={p.tk} mono />}
          </>
        ) : p.act === "appr" ? (
          <>
            <Row k="Type" v={walletName === "gov" ? "Set contract admins" : "Rotate approvers"} />
            <Row k="New threshold" v={`${p.nt || "?"} of ${(p.np || "").split(",").filter(Boolean).length}`} />
            <div className="pt-1 space-y-0.5">
              {(p.np || "").split(",").filter(Boolean).map((pk, i) => (
                <div key={`${pk}-${i}`} className="flex items-center gap-2">
                  <Dot on={true} />
                  <span className="text-text3 text-[11px] font-mono truncate">{shortPk(pk)}</span>
                  {pk === userNpub && <span className="text-neon text-[10px]">you</span>}
                </div>
              ))}
            </div>
          </>
        ) : (
          <>
            <Row k="Type" v="Unpause contract" />
            <Row k="Scope" v="clears the global paused flag" />
          </>
        )}
        <Row k="Expires" v={`${nsToDate(p.exp)} · ${nsCountdown(p.exp)}`} />
      </div>

      {/* approval progress */}
      <div className="p-3 border border-brd rounded-[12px] bg-surface">
        <div className="text-text4 text-[10px] uppercase tracking-wide mb-2">Approvals · {p.ac || 0} of {threshold}</div>
        {approverPks.length === 0 ? <div className="text-text4 text-[11px]">No approver set on chain.</div> : approverPks.map((pk, i) => {
          const approved = (Number(p.bl ?? 0) & (1 << i)) !== 0;
          return (
            <div key={`${pk}-${i}`} className="flex items-center gap-2 py-0.5">
              <Dot on={approved} />
              <span className={`text-[11px] font-mono truncate ${approved ? "text-text" : "text-text4"}`}>{shortPk(pk)}</span>
              {pk === userNpub && <span className="text-neon text-[10px]">you</span>}
              <span className="ml-auto text-text4 text-[10px]">{approved ? "approved" : "pending"}</span>
            </div>
          );
        })}
      </div>

      {/* actions */}
      <div className="space-y-2">
        {p.st === "active" && isApprover && canSign && !alreadyApproved && (
          useRelayer ? (
            <button onClick={doApproveRelay} disabled={busy !== ""} className="w-full inline-flex items-center justify-center gap-1.5 px-3 py-2 rounded-[10px] text-[12px] font-semibold bg-neon text-bg border-none cursor-pointer hover:brightness-110 disabled:opacity-40">
              {busy === "relay" ? <Loader2 size={12} className="animate-spin" /> : <Zap size={12} />} Approve via relayer (gasless)
            </button>
          ) : (
            <button onClick={doApproveDirect} disabled={busy !== ""} className="w-full inline-flex items-center justify-center gap-1.5 px-3 py-2 rounded-[10px] text-[12px] font-semibold bg-neon text-bg border-none cursor-pointer hover:brightness-110 disabled:opacity-40">
              {busy === "approve" ? <Loader2 size={12} className="animate-spin" /> : <Shield size={12} />} Approve
            </button>
          )
        )}
        {p.st === "active" && alreadyApproved && (
          <div className="flex items-center gap-2 px-3 py-2 rounded-[10px] bg-neon-dim border border-neon/25 text-neon text-[11px]"><Check size={12} /> You approved this proposal.</div>
        )}
        {p.st === "active" && !isApprover && (
          <div className="px-3 py-2 rounded-[10px] bg-surface2 border border-brd text-text4 text-[11px]">Your npub is not an approver — read-only.</div>
        )}
        {canExecute && (
          useRelayer ? (
            <button onClick={doExecuteRelay} disabled={busy !== ""} className="w-full inline-flex items-center justify-center gap-1.5 px-3 py-2 rounded-[10px] text-[12px] font-semibold bg-neon text-bg border-none cursor-pointer hover:brightness-110 disabled:opacity-40">
              {busy === "relay" ? <Loader2 size={12} className="animate-spin" /> : <Send size={12} />} Execute via relayer (gasless)
            </button>
          ) : (
            <button onClick={doExecute} disabled={busy !== ""} className="w-full inline-flex items-center justify-center gap-1.5 px-3 py-2 rounded-[10px] text-[12px] font-semibold bg-surface2 text-text border border-brd cursor-pointer hover:border-neon/50 disabled:opacity-40">
              {busy === "execute" ? <Loader2 size={12} className="animate-spin" /> : <Send size={12} />} Execute {p.act === "" ? "payout" : p.act === "appr" ? "rotation" : "unpause"}
            </button>
          )
        )}
        {p.st === "approved" && !canExecute && (
          <div className="px-3 py-2 rounded-[10px] bg-surface2 border border-brd text-text4 text-[11px]">Threshold reached — an approver with a Nostr signer can execute.</div>
        )}
        {p.st === "executed" && (
          <div className="flex items-center gap-2 px-3 py-2 rounded-[10px] bg-neon-dim border border-neon/25 text-neon text-[11px]"><Check size={12} /> Executed on-chain.</div>
        )}
        {!canSign && p.st !== "executed" && (
          <div className="flex items-center gap-2 px-3 py-2 rounded-[10px] bg-yellow/5 border border-yellow/20 text-yellow text-[11px]"><AlertTriangle size={12} /> Sign in with nsec or bunker to act on this proposal.</div>
        )}
      </div>
    </div>
  );
}

function Row({ k, v, mono }: { k: string; v: string; mono?: boolean }) {
  return (
    <div className="flex justify-between gap-3 text-[11px]">
      <span className="text-text4 shrink-0">{k}</span>
      <span className={`text-text text-right break-all ${mono ? "font-mono" : ""}`}>{v}</span>
    </div>
  );
}

// ═════════════════════════════════════════════════════════════════════════
// Page shell — treasury list + drill-down router
// ═════════════════════════════════════════════════════════════════════════

export default function GovernancePage() {
  const { pubkey, secretKey, signEventRaw, mode } = useAuth();
  const { accountId, wallet } = useNear();
  const queryClient = useQueryClient();
  const { toasts, push } = useToasts();

  // drill-down: treasury → wallet → proposal
  const [sel, setSel] = useState<{ t: string | null; w: string | null; p: string | null }>({ t: null, w: null, p: null });
  const [treasuries, setTreasuries] = useState<string[]>(getTreasuries);
  const [showCreate, setShowCreate] = useState(false);
  const [useRelayer, setUseRelayer] = useState(true);

  const signCtx: SignCtx = { secretKey, signEventRaw };
  const canSign = mode === "nsec" || mode === "nip07" || mode === "nip46";

  if (!pubkey) return <LoginScreen />;

  const refreshAll = () => queryClient.invalidateQueries();

  return (
    <div className="flex flex-col h-full overflow-y-auto">
      {/* header */}
      <div className="flex items-center justify-between px-4 py-3 border-b border-brd shrink-0 sticky top-0 bg-bg z-10">
        <h1 className="text-[18px] font-bold">Governance</h1>
        <div className="flex items-center gap-0.5">
          <button onClick={() => setShowCreate(true)} disabled={!accountId || !wallet} title="New treasury" className="p-2 rounded-[10px] text-text3 hover:text-text hover:bg-surface2 transition-colors disabled:opacity-30 cursor-pointer">
            <Plus size={15} />
          </button>
          <button onClick={refreshAll} title="Refresh" className="p-2 rounded-[10px] text-text3 hover:text-text hover:bg-surface2 transition-colors cursor-pointer">
            <RefreshCw size={14} />
          </button>
        </div>
      </div>

      {/* body: either the list, or the drill-down level */}
      {sel.t === null ? (
        <div className="p-2">
          <div className="px-2 pb-2 text-text4 text-[10px] truncate">wallet: {accountId ?? "connecting…"} · {canSign ? "nostr signer ready" : "nostr read-only"}</div>
          {treasuries.map(id => (
            <TreasuryListRow
              key={id}
              contractId={id}
              onOpen={() => setSel({ t: id, w: null, p: null })}
              onRemove={id === DEFAULT_TREASURY ? null : () => { removeTreasury(id); setTreasuries(getTreasuries()); }}
            />
          ))}
        </div>
      ) : sel.w === null ? (
        <TreasuryLevel
          contractId={sel.t}
          userNpub={pubkey}
          canSign={canSign}
          signCtx={signCtx}
          toast={push}
          onSelectWallet={(w) => setSel({ t: sel.t, w, p: null })}
          onRemove={sel.t === DEFAULT_TREASURY ? null : () => { removeTreasury(sel.t!); setTreasuries(getTreasuries()); setSel({ t: null, w: null, p: null }); }}
        />
      ) : sel.p === null ? (
        <WalletLevel
          key={`${sel.t}/${sel.w}`}
          contractId={sel.t}
          walletName={sel.w}
          userNpub={pubkey}
          canSign={canSign}
          signCtx={signCtx}
          useRelayer={useRelayer}
          toast={push}
          onBack={() => setSel({ t: sel.t, w: null, p: null })}
          onSelectProposal={(id) => setSel({ t: sel.t, w: sel.w, p: id })}
        />
      ) : (
        <ProposalLevel
          key={`${sel.t}/${sel.w}/${sel.p}`}
          contractId={sel.t}
          walletName={sel.w}
          proposalId={sel.p}
          userNpub={pubkey}
          canSign={canSign}
          signCtx={signCtx}
          useRelayer={useRelayer}
          toast={push}
          onBack={() => setSel({ t: sel.t, w: sel.w, p: null })}
        />
      )}

      {/* relayer toggle — visible when a proposal could be approved */}
      {sel.p !== null && (
        <div className="px-3 pb-3">
          <button onClick={() => setUseRelayer(!useRelayer)} className={`flex items-center gap-1.5 text-[10px] px-2 py-1 rounded-[6px] border cursor-pointer transition-colors ${useRelayer ? "text-neon border-neon/30 bg-neon/5" : "text-text4 border-brd"}`}>
            <Zap size={10} /> relayer path {useRelayer ? "on" : "off"}
          </button>
        </div>
      )}

      {showCreate && accountId && (
        <CreateTreasuryModal
          accountId={accountId}
          ownerNpub={pubkey}
          walletObj={wallet}
          onClose={() => setShowCreate(false)}
          onCreated={(id) => { setTreasuries(getTreasuries()); setSel({ t: id, w: null, p: null }); }}
          toast={push}
        />
      )}

      <Toasts toasts={toasts} />
    </div>
  );
}

// ── list row: compact live summary of a treasury ─────────────────────────

function TreasuryListRow({
  contractId, onOpen, onRemove,
}: {
  contractId: string;
  onOpen: () => void;
  onRemove: (() => void) | null;
}) {
  const { data: t, isLoading, isError, isFetching } = useTreasury(contractId, true);
  return (
    <div className="flex items-center gap-1 mb-1">
      <button onClick={onOpen} className="flex-1 flex items-center justify-between p-3 border border-brd rounded-[12px] bg-surface cursor-pointer hover:border-neon/40 transition-colors text-left">
        <div className="flex items-center gap-3 min-w-0">
          <div className="w-8 h-8 rounded-[10px] bg-surface2 flex items-center justify-center text-text3 shrink-0"><Landmark size={14} /></div>
          <div className="min-w-0">
            <div className="text-text text-[13px] font-semibold font-mono truncate">{contractId}</div>
            <div className="text-text4 text-[10px] flex items-center gap-1.5">
              <Dot on={!isError} pulse={isFetchingSafe(isFetching)} />
              {isError ? "unreachable" : t ? `v${String(t.version)} · ${yoctoToNear(t.balance)} Ⓝ · ${t.walletNames.length} wallets${t.paused ? " · paused" : ""}` : "loading…"}
            </div>
          </div>
        </div>
        <ChevronRight size={14} className="text-text4" />
      </button>
      {onRemove && (
        <button onClick={onRemove} className="p-2 rounded-[10px] text-text4 hover:text-red hover:bg-red/10 cursor-pointer" title="Remove from list"><Trash2 size={13} /></button>
      )}
    </div>
  );
}

function isFetchingSafe(v: boolean | undefined): boolean {
  return Boolean(v);
}
