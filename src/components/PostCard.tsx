import { ExternalLink, BadgeCheck } from 'lucide-react';
import type { Event } from 'nostr-tools';
import type { UserProfile } from '../lib/types';
import { timeAgo, npubFromHex, noteLink } from '../lib/nostr';

interface PostCardProps {
  event: Event;
  profile: UserProfile | undefined;
  isYou: boolean;
  nearAccount: string | undefined;
}

export function PostCard({ event, profile, isYou, nearAccount }: PostCardProps) {
  const name = profile?.display_name || profile?.name || npubFromHex(event.pubkey).slice(0, 12) + '…';
  const picture = profile?.picture;
  const initial = name[0]?.toUpperCase() ?? '?';
  const verified = nearAccount !== undefined;

  return (
    <div className="rounded-[14px] border border-brd bg-surface p-5">
      {/* Header */}
      <div className="flex items-start gap-3">
        {/* Avatar */}
        {picture ? (
          <img
            src={picture}
            alt={name}
            className="h-10 w-10 shrink-0 rounded-full object-cover bg-surface2"
          />
        ) : (
          <div className="flex h-10 w-10 shrink-0 items-center justify-center rounded-full bg-surface2 text-sm font-medium text-text2">
            {initial}
          </div>
        )}

        <div className="min-w-0 flex-1">
          {/* Name row */}
          <div className="flex flex-wrap items-center gap-1.5">
            <span className={`text-sm font-medium ${isYou ? 'text-neon' : 'text-text'}`}>
              {name}
            </span>
            {isYou && (
              <span className="rounded-md bg-neon/10 px-1.5 py-0.5 text-[10px] font-medium text-neon">
                you
              </span>
            )}
            {verified && (
              <span className="inline-flex items-center gap-0.5 rounded-md bg-neon/10 px-1.5 py-0.5 text-[10px] font-medium text-neon">
                <BadgeCheck className="h-3 w-3" />
                nostr
              </span>
            )}
          </div>

          {/* Content */}
          <p className="mt-1.5 whitespace-pre-wrap break-words text-sm leading-relaxed text-text">
            {event.content}
          </p>

          {/* Footer */}
          <div className="mt-3 flex flex-wrap items-center gap-x-3 gap-y-1">
            <span className="text-xs text-text3">
              {timeAgo(event.created_at)}
            </span>

            {nearAccount && (
              <span className="rounded-md bg-surface2 px-1.5 py-0.5 text-[10px] font-medium text-neon">
                {nearAccount}
              </span>
            )}

            <a
              href={noteLink(event.id)}
              target="_blank"
              rel="noopener noreferrer"
              className="inline-flex items-center gap-1 text-xs text-text3 transition-colors hover:text-text2"
            >
              <ExternalLink className="h-3 w-3" />
              njump
            </a>
          </div>
        </div>
      </div>
    </div>
  );
}
