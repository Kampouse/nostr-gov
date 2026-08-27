export function FeedSkeleton() {
  return (
    <div className="space-y-4">
      {Array.from({ length: 3 }).map((_, i) => (
        <div key={i} className="rounded-[14px] border border-brd bg-surface p-5 animate-pulse-skeleton">
          <div className="flex items-start gap-3">
            {/* Avatar */}
            <div className="h-10 w-10 shrink-0 rounded-full bg-surface2" />
            <div className="flex-1 space-y-2">
              {/* Name row */}
              <div className="flex items-center gap-2">
                <div className="h-4 w-24 rounded bg-surface2" />
                <div className="h-3 w-12 rounded bg-surface2" />
              </div>
              {/* Content lines */}
              <div className="space-y-1.5">
                <div className="h-3 w-full rounded bg-surface2" />
                <div className="h-3 w-4/5 rounded bg-surface2" />
                <div className="h-3 w-3/5 rounded bg-surface2" />
              </div>
              {/* Footer */}
              <div className="flex items-center gap-3 pt-1">
                <div className="h-3 w-14 rounded bg-surface2" />
                <div className="h-3 w-20 rounded bg-surface2" />
              </div>
            </div>
          </div>
        </div>
      ))}
    </div>
  );
}

export function ProfileSkeleton() {
  return (
    <div className="rounded-[14px] border border-brd bg-surface p-5 animate-pulse-skeleton">
      <div className="flex items-center gap-4">
        {/* Avatar */}
        <div className="h-16 w-16 shrink-0 rounded-full bg-surface2" />
        <div className="flex-1 space-y-2">
          {/* Name */}
          <div className="h-5 w-32 rounded bg-surface2" />
          {/* Npub */}
          <div className="h-3 w-48 rounded bg-surface2" />
          {/* About */}
          <div className="h-3 w-full rounded bg-surface2" />
          <div className="h-3 w-3/4 rounded bg-surface2" />
        </div>
      </div>
      {/* Stats row */}
      <div className="mt-4 flex gap-6">
        <div className="space-y-1">
          <div className="h-4 w-8 rounded bg-surface2" />
          <div className="h-3 w-16 rounded bg-surface2" />
        </div>
        <div className="space-y-1">
          <div className="h-4 w-8 rounded bg-surface2" />
          <div className="h-3 w-16 rounded bg-surface2" />
        </div>
        <div className="space-y-1">
          <div className="h-4 w-8 rounded bg-surface2" />
          <div className="h-3 w-16 rounded bg-surface2" />
        </div>
      </div>
    </div>
  );
}
