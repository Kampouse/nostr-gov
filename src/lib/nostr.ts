/**
 * nostr.ts — Shared relay pool, profile cache, feed queries, publish
 * Adapted from nostr-linkedin/lib/nostr.ts
 */

import { SimplePool, nip19 } from "nostr-tools";
import type { Event } from "nostr-tools";
import { READ_RELAYS, WRITE_RELAYS, NIP46_RELAYS } from "./constants";
import type { UserProfile } from "./types";

export type { Event };

// ── Shared pool ──
export const pool = new SimplePool();

// ── Helpers ──

export function shortenPubkey(hex: string): string {
  return hex.slice(0, 8) + "…" + hex.slice(-4);
}

export function npubFromHex(hex: string): string {
  try { return nip19.npubEncode(hex); }
  catch { return shortenPubkey(hex); }
}

export function hexFromNpub(npub: string): string {
  try {
    const decoded = nip19.decode(npub);
    if (decoded.type === "npub") return decoded.data as string;
    return npub;
  } catch { return npub; }
}

export function timeAgo(ts: number): string {
  const diff = Math.floor(Date.now() / 1000) - ts;
  if (diff < 60) return "now";
  if (diff < 3600) return `${Math.floor(diff / 60)}m`;
  if (diff < 86400) return `${Math.floor(diff / 3600)}h`;
  if (diff < 604800) return `${Math.floor(diff / 86400)}d`;
  return new Date(ts * 1000).toLocaleDateString();
}

export function getTagValue(event: Event, tagName: string): string | undefined {
  return event.tags.find((t) => t[0] === tagName)?.[1];
}

// ── Profile cache (in-memory + localStorage) ──
const PROFILE_TTL = 10 * 60 * 1000;
const LS_KEY = "nostrgov_profiles";

interface CachedProfile { profile: UserProfile; fetched: number; }
const profileCache = new Map<string, CachedProfile>();

try {
  const raw = localStorage.getItem(LS_KEY);
  if (raw) {
    const parsed = JSON.parse(raw) as Record<string, CachedProfile>;
    for (const [pk, entry] of Object.entries(parsed)) profileCache.set(pk, entry);
  }
} catch {}

function persistProfiles() {
  try {
    const obj: Record<string, CachedProfile> = {};
    for (const [pk, entry] of [...profileCache.entries()].slice(-200)) obj[pk] = entry;
    localStorage.setItem(LS_KEY, JSON.stringify(obj));
  } catch {}
}

export async function fetchProfile(pubkey: string): Promise<UserProfile> {
  const cached = profileCache.get(pubkey);
  if (cached && Date.now() - cached.fetched < PROFILE_TTL) return cached.profile;

  const events = await fastQuery(
    READ_RELAYS,
    { kinds: [0], authors: [pubkey], limit: 1 },
    1500,
  );

  const profile: UserProfile = {};
  if (events.length > 0) {
    try { Object.assign(profile, JSON.parse(events[0].content)); } catch {}
  }

  profileCache.set(pubkey, { profile, fetched: Date.now() });
  persistProfiles();
  return profile;
}

export async function fetchProfiles(pubkeys: string[]): Promise<Map<string, UserProfile>> {
  const map = new Map<string, UserProfile>();
  const toFetch: string[] = [];

  for (const pk of pubkeys) {
    const cached = profileCache.get(pk);
    if (cached && Date.now() - cached.fetched < PROFILE_TTL) {
      map.set(pk, cached.profile);
    } else {
      toFetch.push(pk);
    }
  }

  if (toFetch.length > 0) {
    const events = await fastQuery(READ_RELAYS, { kinds: [0], authors: toFetch, limit: toFetch.length }, 4000);
    for (const ev of events) {
      const profile: UserProfile = {};
      try { Object.assign(profile, JSON.parse(ev.content)); } catch {}
      profileCache.set(ev.pubkey, { profile, fetched: Date.now() });
      map.set(ev.pubkey, profile);
    }
    for (const pk of toFetch) { if (!map.has(pk)) map.set(pk, {}); }
    persistProfiles();
  }

  return map;
}

// ── Feed ──
export async function fetchGovernanceFeed(limit = 50): Promise<Event[]> {
  const events = await pool.querySync(READ_RELAYS, {
    kinds: [1],
    "#t": ["nostrgov"],
    limit,
  });
  const seen = new Set<string>();
  return events
    .filter((e) => { if (seen.has(e.id)) return false; seen.add(e.id); return true; })
    .sort((a, b) => b.created_at - a.created_at);
}

// ── Reactions (kind 7) ──
export async function fetchReactions(eventIds: string[]): Promise<Map<string, { count: number; mine: boolean }>> {
  // TODO: batch fetch kind 7 for eventIds when needed
  return new Map();
}

// ── Publish ──
export async function publishEvent(signedEvent: Event): Promise<void> {
  const results = await Promise.allSettled(
    [...WRITE_RELAYS, ...NIP46_RELAYS].map((r) => pool.publish([r], signedEvent)),
  );
  const ok = results.filter((r) => r.status === "fulfilled").length;
  console.log(`[nostr] published to ${ok}/${results.length} relays`);
}

// ── fastQuery — race relays, return on first EOSE or first event ──
export async function fastQuery(
  relays: string[],
  filter: { kinds: number[]; authors?: string[]; limit?: number; since?: number },
  timeoutMs = 1500,
): Promise<Event[]> {
  return new Promise((resolve) => {
    const collected: Event[] = [];
    const seen = new Set<string>();

    const sub = pool.subscribeMany(relays, filter, {
      onevent(e: Event) {
        if (!seen.has(e.id)) { seen.add(e.id); collected.push(e); }
        if (filter.kinds[0] === 0 || filter.kinds[0] === 3) {
          sub.close(); resolve(collected);
        }
      },
      oneose() { sub.close(); resolve(collected); },
    });

    setTimeout(() => { sub.close(); resolve(collected); }, timeoutMs);
  });
}

// ── Note link ──
export function noteLink(eventId: string, hint?: string): string {
  try {
    const relays = hint ? [hint, ...READ_RELAYS] : READ_RELAYS.slice(0, 2);
    return "https://njump.me/" + nip19.neventEncode({ id: eventId, relays });
  } catch { return `https://njump.me/${eventId}`; }
}
