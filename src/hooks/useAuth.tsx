/**
 * useAuth.tsx — Auth context: NIP-46, NIP-07, nsec, npub
 * Adapted from nostr-linkedin/hooks/useAuth.tsx
 */

import { createContext, useContext, useState, useEffect, useCallback, type ReactNode } from "react";
import { hexToBytes } from "../lib/bytes";
import { generateSecretKey, getPublicKey, finalizeEvent, nip19 } from "nostr-tools";
import { startNdkConnect, NdkNostrSigner, type NdkConnectHandle } from "../lib/ndk-signer";
import { NIP46_RELAYS } from "../lib/constants";
import { fetchProfile, publishEvent, npubFromHex } from "../lib/nostr";
import type { UserProfile } from "../lib/types";

const STORAGE_KEY = "nostrgov:session";

export interface AuthState {
  signer: NdkNostrSigner | null;
  pubkey: string;
  npub: string;
  profile: UserProfile | null;
  loading: boolean;
  error: string;
  readOnly: boolean;
  secretKey: Uint8Array | null;
  sessionAlive: boolean;
  mode: string;
  // NIP-46
  connectUri: string;
  connectHandle: NdkConnectHandle | null;
  startConnect: () => void;
  cancelConnect: () => void;
  // Login methods
  loginNip07: () => Promise<void>;
  loginNsec: (nsec: string) => void;
  loginNpub: (npub: string) => void;
  // Shared
  disconnect: () => void;
  refreshProfile: () => Promise<void>;
  signAndPublish: (template: { kind: number; content: string; tags?: string[][] }) => Promise<void>;
  signEventRaw: (template: { kind: number; content: string; tags: string[][] }) => Promise<any>;
}

const AuthContext = createContext<AuthState | null>(null);

export function useAuth(): AuthState {
  const ctx = useContext(AuthContext);
  if (!ctx) throw new Error("useAuth must be used within AuthProvider");
  return ctx;
}

export function AuthProvider({ children }: { children: ReactNode }) {
  const [signer, setSigner] = useState<NdkNostrSigner | null>(null);
  const [pubkey, setPubkey] = useState("");
  const [profile, setProfile] = useState<UserProfile | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const [connectUri, setConnectUri] = useState("");
  const [connectHandle, setConnectHandle] = useState<NdkConnectHandle | null>(null);
  const [readOnly, setReadOnly] = useState(false);
  const [secretKey, setSecretKey] = useState<Uint8Array | null>(null);
  const [sessionAlive, setSessionAlive] = useState(true);
  const [authMode, setAuthMode] = useState("");

  const refreshProfile = useCallback(async () => {
    if (!pubkey) return;
    const p = await fetchProfile(pubkey);
    setProfile(p);
  }, [pubkey]);

  useEffect(() => { if (pubkey) refreshProfile(); }, [pubkey, refreshProfile]);

  // ── Restore session ──
  useEffect(() => {
    const saved = localStorage.getItem(STORAGE_KEY);
    if (!saved) return;
    try {
      const parsed = JSON.parse(saved);
      if (parsed.mode === "npub" && parsed.pubkey) {
        setPubkey(parsed.pubkey);
        setReadOnly(true); setAuthMode("npub");
        return;
      }
      if (parsed.nsecHex) {
        const sk = hexToBytes(parsed.nsecHex);
        setSecretKey(sk);
        setPubkey(getPublicKey(sk)); setAuthMode("nsec");
        return;
      }
      if (parsed.clientSecKey && parsed.bunkerPubkey) {
        const s = new NdkNostrSigner({
          clientSecretKey: hexToBytes(parsed.clientSecKey),
          bunkerPubkey: parsed.bunkerPubkey,
          relays: parsed.relays || NIP46_RELAYS,
          userPubkey: parsed.userPubkey,
        });
        setSigner(s);
        setPubkey(parsed.userPubkey || parsed.bunkerPubkey);
      }
    } catch (e: unknown) {
      console.warn("[auth] restore failed:", e instanceof Error ? e.message : e);
      localStorage.removeItem(STORAGE_KEY);
    }
  }, []);

  // ── Session health check (NIP-46) ──
  useEffect(() => {
    if (!signer || secretKey) return;
    let cancelled = false;
    setSessionAlive(false);
    const check = async () => {
      if (cancelled) return;
      try {
        const alive = await signer.ping();
        if (cancelled) return;
        if (!alive) { disconnect(); } else { setSessionAlive(true); }
      } catch { if (!cancelled) disconnect(); }
    };
    const initial = setTimeout(check, 3_000);
    const interval = setInterval(check, 120_000);
    return () => { cancelled = true; clearTimeout(initial); clearInterval(interval); };
  }, [signer, secretKey]);

  // ── Sign + publish ──
  const signAndPublish = useCallback(async (template: { kind: number; content: string; tags?: string[][] }) => {
    const saved = localStorage.getItem(STORAGE_KEY);
    const parsed = saved ? JSON.parse(saved) : null;

    // NIP-07 extension
    if (parsed?.mode === "nip07") {
      const evt = await (window as any).nostr.signEvent({
        kind: template.kind, content: template.content, tags: template.tags || [],
        created_at: Math.floor(Date.now() / 1000),
      });
      await publishEvent(evt);
      return;
    }

    // nsec local
    if (secretKey) {
      const evt = finalizeEvent(
        { kind: template.kind, content: template.content, tags: template.tags || [], created_at: Math.floor(Date.now() / 1000) },
        secretKey,
      );
      await publishEvent(evt);
      return;
    }

    // NIP-46 remote
    if (signer) {
      const evt = await signer.signEvent({
        kind: template.kind, content: template.content, tags: template.tags || [],
        created_at: Math.floor(Date.now() / 1000),
      });
      await publishEvent(evt);
      return;
    }

    throw new Error("No signer available");
  }, [signer, secretKey]);

  const signEventRaw = useCallback(async (template: { kind: number; content: string; tags: string[][] }) => {
    if (secretKey) {
      return finalizeEvent(
        { kind: template.kind, content: template.content, tags: template.tags, created_at: Math.floor(Date.now() / 1000) },
        secretKey,
      );
    }
    if (signer) {
      return signer.signEvent({
        kind: template.kind, content: template.content, tags: template.tags,
        created_at: Math.floor(Date.now() / 1000),
      });
    }
    throw new Error("No signer available");
  }, [signer, secretKey]);

  // ── NIP-46 pairing ──
  const startConnect = useCallback(() => {
    setError("");
    setLoading(true);
    let handle: NdkConnectHandle;
    try {
      handle = startNdkConnect({
        relays: NIP46_RELAYS,
        perms: "get_public_key,nip04_encrypt,nip04_decrypt,nip44_encrypt,nip44_decrypt,sign_event:0,sign_event:1,sign_event:27235,sign_event:37500",
        metadata: { name: "NostrGov", url: "https://nostr-gov.pages.dev" },
      });
    } catch (e: unknown) {
      setError("Init failed: " + (e instanceof Error ? e.message : e));
      setLoading(false);
      return;
    }

    setConnectHandle(handle);
    setConnectUri(handle.uri);

    handle.ready
      .then(async (s) => {
        let userPk: string | null = null;
        try { userPk = await s.getPublicKey(); }
        catch { userPk = (s as any)._userPubkey || null; }

        if (!userPk) { setError("Bunker did not return a public key."); setLoading(false); return; }

        const serialized = s.serialize();
        localStorage.setItem(STORAGE_KEY, JSON.stringify({
          mode: "nip46",
          clientSecKey: serialized.clientSecretKey,
          bunkerPubkey: serialized.bunkerPubkey,
          relays: serialized.relays,
          userPubkey: serialized.userPubkey,
        }));

        setSigner(s);
        setPubkey(userPk);
        setConnectUri("");
        setConnectHandle(null);
        setLoading(false);
      })
      .catch((e: unknown) => {
        setError(e instanceof Error ? e.message : "Pairing failed");
        setConnectUri("");
        setConnectHandle(null);
        setLoading(false);
      });
  }, []);

  const cancelConnect = useCallback(() => {
    connectHandle?.cancel();
    setConnectHandle(null);
    setConnectUri("");
    setLoading(false);
  }, [connectHandle]);

  // ── NIP-07 ──
  const loginNip07 = useCallback(async () => {
    if (!(window as any).nostr) {
      setError("No NIP-07 extension found. Install Alby or nos2x.");
      return;
    }
    setError("");
    setLoading(true);
    try {
      const pk = await (window as any).nostr.getPublicKey();
      if (!pk) throw new Error("Extension denied access");
      localStorage.setItem(STORAGE_KEY, JSON.stringify({ mode: "nip07", pubkey: pk }));
      setPubkey(pk);
      setReadOnly(false);
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : "NIP-07 login failed");
    } finally { setLoading(false); }
  }, []);

  // ── nsec ──
  const loginNsec = useCallback((nsecOrHex: string) => {
    setError("");
    setLoading(true);
    try {
      let sk: Uint8Array;
      if (nsecOrHex.startsWith("nsec")) {
        const decoded = nip19.decode(nsecOrHex);
        if (decoded.type !== "nsec") throw new Error("Invalid nsec");
        sk = decoded.data as Uint8Array;
      } else {
        sk = hexToBytes(nsecOrHex);
      }
      const pk = getPublicKey(sk);
      const hexSk = Array.from(sk).map(b => b.toString(16).padStart(2, "0")).join("");
      localStorage.setItem(STORAGE_KEY, JSON.stringify({ mode: "nsec", nsecHex: hexSk }));
      setSecretKey(sk);
      setPubkey(pk);
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : "Invalid key");
    } finally { setLoading(false); }
  }, []);

  // ── npub (read-only) ──
  const loginNpub = useCallback((npubOrHex: string) => {
    setError("");
    setLoading(true);
    try {
      let pk: string;
      if (npubOrHex.startsWith("npub")) {
        const decoded = nip19.decode(npubOrHex);
        if (decoded.type !== "npub") throw new Error("Invalid npub");
        pk = decoded.data as string;
      } else { pk = npubOrHex; }
      localStorage.setItem(STORAGE_KEY, JSON.stringify({ mode: "npub", pubkey: pk }));
      setPubkey(pk);
      setReadOnly(true);
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : "Invalid key");
    } finally { setLoading(false); }
  }, []);

  // ── Disconnect ──
  const disconnect = useCallback(() => {
    try { signer?.close(); } catch { /* already dead */ }
    setSigner(null);
    setPubkey("");
    setProfile(null);
    setError("");
    setReadOnly(false);
    setSecretKey(null);
    setSessionAlive(true);
    localStorage.removeItem(STORAGE_KEY);
  }, [signer]);

  const npub = pubkey ? npubFromHex(pubkey) : "";

  return (
    <AuthContext.Provider value={{
      signer, pubkey, npub, profile, loading, error, readOnly,
      secretKey, sessionAlive, mode: authMode,
      connectUri, connectHandle, startConnect, cancelConnect,
      loginNip07, loginNsec, loginNpub,
      disconnect, refreshProfile,
      signAndPublish, signEventRaw,
    }}>
      {children}
    </AuthContext.Provider>
  );
}
