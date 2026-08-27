/**
 * IdentityPage.tsx — Nostr identity, NEAR wallet connect, binding
 */

import { useState, useEffect } from "react";
import { Shield, ShieldCheck, Copy, Check, Wallet, LogOut, ExternalLink, RotateCcw } from "lucide-react";
import { useAuth } from "../hooks/useAuth";
import { useNear } from "../hooks/useNearWallet";
import { fetchAllBindingsCached } from "../lib/binding";
import { useQuery } from "@tanstack/react-query";
import { LoginScreen } from "../components/LoginScreen";
import { NEAR_RPC } from "../lib/constants";

export default function IdentityPage() {
  const { pubkey, npub, profile, mode: authMode, sessionAlive, secretKey, disconnect } = useAuth();
  const { accountId, connecting, connect, disconnect: nearDisconnect, publicKey } = useNear();
  const [copied, setCopied] = useState("");
  const [nearCopied, setNearCopied] = useState(false);
  const [bindingAccount, setBindingAccount] = useState<string | null>(null);

  // Reverse-lookup NEAR account by npub from binding cache
  const { data: _bindings } = useQuery({
    queryKey: ["all-bindings"],
    queryFn: fetchAllBindingsCached,
    staleTime: 5 * 60_000,
  });

  useEffect(() => {
    if (npub && _bindings?.pubkeyIndex) {
      setBindingAccount(_bindings.pubkeyIndex[npub] ?? null);
    } else {
      setBindingAccount(null);
    }
  }, [npub, _bindings]);

  // Query NEAR account balance
  const { data: balance } = useQuery({
    queryKey: ["near-balance", accountId],
    queryFn: async () => {
      const res = await fetch(NEAR_RPC, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          jsonrpc: "2.0", id: 1,
          method: "query",
          params: { request_type: "view_account", finality: "final", account_id: accountId },
        }),
      });
      const json = await res.json();
      return (json.result?.amount ?? "0") as string;
    },
    enabled: !!accountId,
    staleTime: 30_000,
  });

  const copy = (text: string, which: "npub" | "near") => {
    navigator.clipboard.writeText(text);
    if (which === "npub") { setCopied(text); setTimeout(() => setCopied(""), 2000); }
    else { setNearCopied(true); setTimeout(() => setNearCopied(false), 2000); }
  };

  const fmtNear = (yocto: string) => {
    const val = Number(BigInt(yocto) / 10n ** 21n);
    return val.toFixed(val < 1 ? 4 : 2);
  };

  const modeLabel =
    authMode === "nip46" ? "Nostr Connect"
    : authMode === "nip07" ? "Browser Extension"
    : authMode === "nsec" ? "Local Key (nsec)"
    : authMode === "npub" ? "Read-Only (npub)"
    : "—";

  if (!pubkey) return <LoginScreen />;

  return (
    <div className="flex flex-col h-full overflow-y-auto pb-4">
      {/* Nostr Identity */}
      <div className="m-4 p-5 border border-brd rounded-[14px] bg-surface">
        <h3 className="text-text3 text-[11px] font-bold uppercase tracking-wider flex items-center gap-1.5 mb-3.5">
          <Shield size={14} /> Nostr Identity
        </h3>
        <div className="flex items-center gap-2 bg-bg border border-brd rounded-[10px] px-3 py-2.5 mb-3">
          <code className="flex-1 text-text2 text-[11px] font-mono truncate">{npub}</code>
          <button onClick={() => copy(npub, "npub")} className="text-text4 hover:text-text transition-colors">
            {copied === npub ? <Check size={14} /> : <Copy size={14} />}
          </button>
        </div>
        <div className="flex items-center justify-between">
          <span className="inline-flex items-center gap-1 text-neon text-[11px] font-semibold bg-neon-dim px-2.5 py-1 rounded-full border border-neon/25">
            <ShieldCheck size={12} /> {modeLabel}
          </span>
          <span className="text-text4 text-[10px] font-medium px-2 py-0.5 rounded-full bg-surface2 border border-brd">
            {sessionAlive || secretKey ? "● alive" : "● reconnecting"}
          </span>
        </div>
      </div>

      {/* NEAR Wallet */}
      <div className="mx-4 mb-4 p-5 border border-brd rounded-[14px] bg-surface">
        <h3 className="text-text3 text-[11px] font-bold uppercase tracking-wider flex items-center gap-1.5 mb-3.5">
          <Wallet size={14} /> NEAR Wallet
        </h3>
        {accountId ? (
          <>
            <div className="flex items-center gap-2 bg-bg border border-brd rounded-[10px] px-3 py-2.5 mb-3">
              <span className="text-neon font-semibold text-[13px] truncate">{accountId}</span>
              <button onClick={() => copy(accountId, "near")} className="text-text4 hover:text-text transition-colors shrink-0">
                {nearCopied ? <Check size={14} /> : <Copy size={14} />}
              </button>
            </div>
            {balance && (
              <div className="flex items-center gap-2 mb-3 text-[13px]">
                <span className="text-text4">Balance:</span>
                <span className="text-neon font-mono font-semibold">{fmtNear(balance)} NEAR</span>
              </div>
            )}
            <a
              href={`https://testnet.nearblocks.io/account/${accountId}`}
              target="_blank" rel="noopener noreferrer"
              className="inline-flex items-center gap-1 text-neon text-[11px] hover:underline mb-3"
            >
              <ExternalLink size={11} /> View on NearBlocks
            </a>
            <br />
            <button
              onClick={nearDisconnect}
              className="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-[10px] text-[11px] font-medium text-red border border-red/25 bg-transparent cursor-pointer transition-colors hover:bg-red/10 mt-1"
            >
              <LogOut size={13} /> Disconnect
            </button>
          </>
        ) : (
          <>
            <p className="text-text2 text-[13px] mb-3">Connect for governance signing and npub binding.</p>
            <button
              onClick={connect}
              disabled={connecting}
              className="inline-flex items-center gap-1.5 px-4 py-2.5 rounded-[10px] text-[12px] font-semibold bg-neon text-bg border-none cursor-pointer transition-all hover:brightness-110 disabled:opacity-50 disabled:cursor-wait"
            >
              {connecting ? (
                <span className="inline-block w-3 h-3 border-2 border-bg/30 border-t-bg rounded-full animate-spin" />
              ) : (
                <Wallet size={14} />
              )}
              {connecting ? "Connecting…" : "Connect NEAR Wallet"}
            </button>
          </>
        )}
      </div>

      {/* Binding Status */}
      {accountId && (
        <div className="mx-4 mb-4 p-5 border border-brd rounded-[14px] bg-surface">
          <h3 className="text-text3 text-[11px] font-bold uppercase tracking-wider flex items-center gap-1.5 mb-3.5">
            <ShieldCheck size={14} /> Binding Status
          </h3>
          {bindingAccount ? (
            <div className="flex items-center gap-2 text-[13px]">
              <span className="text-neon font-mono truncate">{bindingAccount}</span>
              <span className="inline-flex items-center gap-1 text-neon text-[11px] font-semibold bg-neon-dim px-2 py-0.5 rounded-full border border-neon/25">
                <ShieldCheck size={11} /> Bound
              </span>
            </div>
          ) : (
            <div className="text-text3 text-[12px]">
              <p className="mb-2">This npub is not yet bound to a NEAR account in the contextual.near registry.</p>
              <p className="text-text4 text-[11px]">Binding requires a transaction on the NEAR side (not yet implemented).</p>
            </div>
          )}
        </div>
      )}

      {/* Profile */}
      {profile && (
        <div className="mx-4 mb-4 p-5 border border-brd rounded-[14px] bg-surface">
          <div className="flex gap-4 items-start">
            {profile.picture && (
              <img src={profile.picture} className="w-[60px] h-[60px] rounded-full border border-brd" alt="" />
            )}
            <div className="flex-1 min-w-0">
              <h2 className="text-[17px] font-semibold">{profile.display_name || profile.name || "—"}</h2>
              {profile.about && <p className="text-text2 text-[13px] mt-1 leading-relaxed">{profile.about}</p>}
              {profile.nip05 && <p className="text-neon text-[11px] font-mono mt-1.5">{profile.nip05}</p>}
              {profile.website && <p className="text-text3 text-[11px] mt-1">{profile.website}</p>}
            </div>
          </div>
        </div>
      )}

      {/* Key Management */}
      <div className="mx-4 mb-4 p-5 border border-brd rounded-[14px] bg-surface">
        <div className="flex items-center justify-between mb-2.5">
          <span className="text-text3 text-[11px] font-bold uppercase tracking-wider">Local Key</span>
          <span className="text-text3 text-[10px] font-semibold px-2 py-0.5 rounded-full bg-surface2 border border-brd">{modeLabel}</span>
        </div>
        <code className="block text-text4 text-[11px] font-mono break-all bg-bg border border-brd rounded-[10px] px-3 py-2.5 mb-3">{pubkey}</code>
        <button
          onClick={disconnect}
          className="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-[10px] text-[11px] font-medium text-red border border-red/25 bg-transparent cursor-pointer transition-colors hover:bg-red/10"
        >
          <RotateCcw size={13} /> Remove key
        </button>
      </div>
    </div>
  );
}
