/**
 * FeedPage.tsx — Real relay feed with composer
 */

import { useState, useRef, useCallback } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Send, MessageSquare } from "lucide-react";
import { useAuth } from "../hooks/useAuth";
import {
  fetchGovernanceFeed,
  fetchProfiles,
  npubFromHex,
  type Event,
} from "../lib/nostr";
import { fetchAllBindingsCached, lookupByPubkey } from "../lib/binding";
import { PostCard } from "../components/PostCard";
import { FeedSkeleton } from "../components/Skeleton";
import { LoginScreen } from "../components/LoginScreen";

export default function FeedPage() {
  const { pubkey, npub, readOnly, signAndPublish } = useAuth();
  const queryClient = useQueryClient();
  const [input, setInput] = useState("");
  const [sending, setSending] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);

  // ── Feed ──
  const { data: events = [], isLoading } = useQuery({
    queryKey: ["feed"],
    queryFn: () => fetchGovernanceFeed(50),
    refetchInterval: 30_000,
  });

  // ── Profiles ──
  const pubkeys = [...new Set(events.map((e) => e.pubkey))];
  const { data: profiles = new Map() } = useQuery({
    queryKey: ["profiles", ...pubkeys.sort()],
    queryFn: () => fetchProfiles(pubkeys),
    enabled: pubkeys.length > 0,
    staleTime: 600_000,
  });

  // ── Bindings (for NEAR account display) ──
  const { data: bindingCache } = useQuery({
    queryKey: ["bindings"],
    queryFn: fetchAllBindingsCached,
    staleTime: 300_000,
  });

  const handleSend = useCallback(async () => {
    const text = input.trim();
    if (!text || readOnly) return;
    setSending(true);
    try {
      await signAndPublish({
        kind: 1,
        content: text,
        tags: [["t", "nostrgov"], ["client", "nostr-gov"]],
      });
      setInput("");
      queryClient.invalidateQueries({ queryKey: ["feed"] });
    } catch (e: any) {
      console.error("[feed] publish failed:", e);
    }
    setSending(false);
    inputRef.current?.focus();
  }, [input, readOnly, signAndPublish, queryClient]);

  if (!pubkey) return <LoginScreen />;

  return (
    <div className="flex flex-col h-full">
      {/* Composer */}
      <div className="px-4 py-3 border-b border-brd shrink-0">
        <div className="flex gap-2">
          <input
            ref={inputRef}
            value={input}
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && !e.shiftKey && handleSend()}
            placeholder={readOnly ? "Read-only mode" : "What are you governing?"}
            disabled={readOnly}
            className="flex-1 bg-surface2 border border-brd rounded-[10px] px-3.5 py-2.5 text-text text-[14px] outline-none placeholder:text-text4 transition-colors disabled:opacity-30 focus:border-neon/50"
          />
          <button
            onClick={handleSend}
            disabled={readOnly || sending || !input.trim()}
            className="bg-neon text-bg w-10 h-10 rounded-[10px] flex items-center justify-center shrink-0 transition-opacity disabled:opacity-30 hover:brightness-110"
          >
            <Send size={18} />
          </button>
        </div>
      </div>

      {/* Feed */}
      <div className="flex-1 overflow-y-auto">
        {isLoading ? (
          <FeedSkeleton />
        ) : events.length === 0 ? (
          <div className="text-center py-16 text-text3 text-[13px]">
            <MessageSquare size={32} className="mx-auto mb-2" />
            <p>No posts yet. Be the first.</p>
          </div>
        ) : (
          events.map((event) => {
            const profile = profiles.get(event.pubkey);
            const eventNpub = npubFromHex(event.pubkey);
            const nearAccount = bindingCache?.pubkeyIndex[eventNpub];
            return (
              <PostCard
                key={event.id}
                event={event}
                profile={profile}
                isYou={event.pubkey === pubkey}
                nearAccount={nearAccount}
              />
            );
          })
        )}
      </div>
    </div>
  );
}
