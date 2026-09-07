import { NavLink } from 'react-router-dom';
import { MessageSquare, Shield, Landmark, WifiOff, LogOut } from 'lucide-react';
import { useAuth } from '../hooks/useAuth';

interface LayoutProps {
  children: React.ReactNode;
  activeTab?: 'feed' | 'identity' | 'governance';
}

const tabs = [
  { to: '/', icon: MessageSquare, label: 'Feed', key: 'feed' },
  { to: '/identity', icon: Shield, label: 'Identity', key: 'identity' },
  { to: '/governance', icon: Landmark, label: 'Governance', key: 'governance' },
] as const;

export function Layout({ children }: LayoutProps) {
  const { sessionAlive, pubkey, npub, disconnect } = useAuth();

  return (
    <div className="flex min-h-dvh flex-col bg-bg">
      {/* Reconnecting banner */}
      {pubkey && !sessionAlive && (
        <div className="flex items-center justify-center gap-2 bg-yellow/10 px-4 py-2 text-xs text-yellow">
          <WifiOff className="h-3.5 w-3.5" />
          Reconnecting…
        </div>
      )}

      {/* Top bar — always-visible logout */}
      {pubkey && (
        <header className="flex h-12 shrink-0 items-center justify-between border-b border-brd bg-surface px-4">
          <div className="flex min-w-0 items-center gap-2">
            <span className="text-[13px] font-bold tracking-tight text-text">nostr-gov</span>
            <span className="truncate font-mono text-[10px] text-text4">{npub.slice(0, 16)}…</span>
          </div>
          <button
            onClick={disconnect}
            title="Log out"
            aria-label="Log out"
            className="flex h-9 w-9 shrink-0 items-center justify-center rounded-[10px] text-text3 transition-colors hover:bg-red/10 hover:text-red active:bg-red/15"
          >
            <LogOut className="h-4.5 w-4.5" />
          </button>
        </header>
      )}

      {/* Main content */}
      <main className="flex-1 overflow-y-auto">
        {children}
      </main>

      {/* Bottom tab bar */}
      <nav className="flex h-16 shrink-0 items-center justify-around border-t border-brd bg-surface">
        {tabs.map(({ to, icon: Icon, label }) => (
          <NavLink
            key={to}
            to={to}
            end={to === '/'}
            className={({ isActive }) =>
              `flex h-11 flex-col items-center justify-center gap-0.5 rounded-xl px-4 text-[10px] font-medium transition-colors ${
                isActive ? 'text-neon' : 'text-text4'
              }`
            }
          >
            <Icon className="h-5 w-5" />
            <span>{label}</span>
          </NavLink>
        ))}
      </nav>
    </div>
  );
}
