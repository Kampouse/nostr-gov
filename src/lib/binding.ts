/**
 * binding.ts — NEAR↔Nostr binding via FastNear KV
 * Adapted from legion-chat/lib/binding.ts
 */

import { KV_ACCOUNTS, FASTNEAR_KV_API } from "./constants";
import type { BindingValue, BindingCache } from "./types";

const CACHE_KEY = "nostrgov:bindings_cache";
const CACHE_TTL_MS = 5 * 60 * 1000;

interface CachedBindings extends BindingCache { cachedAt: number; }

interface KvEntry { key: string; value: unknown; }
interface KvResponse { entries: KvEntry[]; page_token?: string; }

// ── Cache ──

function loadCache(): CachedBindings | null {
  try {
    const raw = localStorage.getItem(CACHE_KEY);
    if (!raw) return null;
    const cached = JSON.parse(raw) as CachedBindings;
    if (Date.now() - cached.cachedAt > CACHE_TTL_MS) return null;
    return cached;
  } catch { return null; }
}

function saveCache(bindings: Record<string, { npub: string; relay: string }>): CachedBindings {
  const pubkeyIndex: Record<string, string> = {};
  for (const [accountId, b] of Object.entries(bindings)) pubkeyIndex[b.npub] = accountId;
  const cached: CachedBindings = { bindings, pubkeyIndex, cachedAt: Date.now() };
  localStorage.setItem(CACHE_KEY, JSON.stringify(cached));
  return cached;
}

// ── KV fetch ──

async function fetchKvEntries(account: string, keyPrefix: string): Promise<KvEntry[]> {
  const all: KvEntry[] = [];
  let pageToken: string | undefined;
  do {
    const body: Record<string, unknown> = { key_prefix: keyPrefix, limit: 200 };
    if (pageToken) body.page_token = pageToken;
    const res = await fetch(`${FASTNEAR_KV_API}/v0/latest/${account}`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    });
    if (!res.ok) { if (res.status === 404) return all; throw new Error(`KV fetch failed: ${res.status}`); }
    const data: KvResponse = await res.json();
    all.push(...data.entries);
    pageToken = data.page_token;
  } while (pageToken);
  return all;
}

// ── Public API ──

export async function fetchBinding(nearAccountId: string): Promise<BindingValue | null> {
  for (const account of KV_ACCOUNTS) {
    const url = `${FASTNEAR_KV_API}/v0/latest/${account}/${nearAccountId}/nostr/${nearAccountId}`;
    try {
      const res = await fetch(url);
      if (!res.ok) continue;
      const data = await res.json();
      if (data?.entries?.length > 0) return data.entries[0].value as BindingValue;
    } catch { /* try next */ }
  }
  return null;
}

export async function fetchAllBindingsCached(): Promise<BindingCache> {
  const cached = loadCache();
  if (cached) return cached;
  return saveCache(await fetchAllBindingsFresh());
}

export async function fetchAllBindingsRefresh(): Promise<BindingCache> {
  return saveCache(await fetchAllBindingsFresh());
}

export function lookupByPubkey(npub: string): string | undefined {
 return loadCache()?.pubkeyIndex[npub];
}

async function fetchAllBindingsFresh(): Promise<Record<string, { npub: string; relay: string }>> {
  const result: Record<string, { npub: string; relay: string }> = {};
  for (const account of KV_ACCOUNTS) {
    try {
      const entries = await fetchKvEntries(account, "nostr/");
      for (const entry of entries) {
        const accountId = entry.key.slice(6);
        try {
          const parsed = entry.value as BindingValue;
          if (parsed?.npub && !result[accountId]) {
            result[accountId] = { npub: parsed.npub, relay: parsed.relay || "" };
          }
        } catch { /* skip */ }
      }
    } catch (e: unknown) {
      console.warn(`[binding] fetchAllBindings (${account}):`, e instanceof Error ? e.message : e);
    }
  }
  return result;
}
