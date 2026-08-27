import { Shield, QrCode, Key, Fingerprint } from 'lucide-react';
import { useState, useEffect, useRef, useCallback } from 'react';
import { QRCodeSVG } from 'qrcode.react';
import { useAuth } from '../hooks/useAuth';

const isMobile = () => /Mobi|Android/i.test(navigator.userAgent);

// ── Pairing progress stepper ──
const PAIR_STEPS = [
  { label: 'Waiting for scan', sub: 'Scan the QR with your signer app' },
  { label: 'Pairing handshake', sub: 'Signer responded over the relay' },
  { label: 'Fetching public key', sub: 'Opening an encrypted channel' },
] as const;

export function LoginScreen() {
  const {
    loading,
    error,
    connectUri,
    startConnect,
    cancelConnect,
    loginNip07,
    loginNsec,
    loginNpub,
  } = useAuth();

  const [screen, setScreen] = useState<'start' | 'qr'>('start');
  const [nsecInput, setNsecInput] = useState('');
  const [npubInput, setNpubInput] = useState('');
  const [elapsed, setElapsed] = useState(0);
  const [pairStep, setPairStep] = useState(0);
  const [logs, setLogs] = useState<string[]>([]);
  const logRef = useRef<HTMLDivElement>(null);

  // Capture NIP-46 logs
  useEffect(() => {
    const orig = console.log;
    console.log = (...args: unknown[]) => {
      orig(...args);
      const text = args.map(a => typeof a === 'string' ? a : JSON.stringify(a)).join(' ');
      if (text.includes('[NIP-46]')) {
        setLogs(prev => [...prev.slice(-80), text]);
      }
    };
    return () => { console.log = orig; };
  }, []);

  // Derive step from logs
  useEffect(() => {
    const all = logs.join(' ');
    if (all.includes('creating NdkNostrSigner') || all.includes('getPublicKey')) setPairStep(p => Math.max(p, 2));
    else if (all.includes('PAIRED') || all.includes('pair response')) setPairStep(p => Math.max(p, 1));
  }, [logs]);

  // Auto-scroll logs
  useEffect(() => {
    const el = logRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [logs]);

  // Timer
  useEffect(() => {
    if (screen !== 'qr') return;
    const start = Date.now();
    const iv = setInterval(() => setElapsed(Math.floor((Date.now() - start) / 1000)), 1000);
    return () => clearInterval(iv);
  }, [screen]);

  const handleStartConnect = useCallback(() => {
    setLogs([]);
    setPairStep(0);
    startConnect();
    setScreen('qr');
  }, [startConnect]);

  const handleCancel = useCallback(() => {
    cancelConnect();
    setScreen('start');
    setLogs([]);
  }, [cancelConnect]);

  const copyText = async (text: string) => {
    try { await navigator.clipboard.writeText(text); } catch {}
  };

  // ── QR SCREEN ──
  if (screen === 'qr' && connectUri) {
    return (
      <div className="flex min-h-dvh flex-col items-center px-5 pb-8 pt-10">
        {/* Status */}
        <div className="mb-6 text-center">
          <div className="mb-2 text-2xl">📱</div>
          <p className="text-base font-semibold text-text">Scan with your signer app</p>
          <p className="mt-1 text-[13px] text-text3">
            {elapsed > 0 ? `Waiting… (${elapsed}s)` : 'Preparing QR…'}
          </p>
        </div>

        {/* QR Code */}
        <div className="relative mb-6 rounded-3xl bg-white p-4 shadow-2xl">
          <QRCodeSVG value={connectUri} size={220} className="rounded-lg" />
          <div className="absolute -left-1 -top-1 h-5 w-5 rounded-tl-xl border-l-2 border-t-2 border-neon/50" />
          <div className="absolute -right-1 -top-1 h-5 w-5 rounded-tr-xl border-r-2 border-t-2 border-neon/50" />
          <div className="absolute -bottom-1 -left-1 h-5 w-5 rounded-bl-xl border-b-2 border-l-2 border-neon/50" />
          <div className="absolute -bottom-1 -right-1 h-5 w-5 rounded-br-xl border-b-2 border-r-2 border-neon/50" />
        </div>

        {/* Stepper */}
        <div className="mb-6 w-full max-w-xs rounded-2xl border border-brd bg-surface/50 p-4">
          <div className="flex flex-col">
            {PAIR_STEPS.map((step, i) => {
              const done = i < pairStep;
              const active = i === pairStep;
              const isLast = i === PAIR_STEPS.length - 1;
              return (
                <div key={step.label} className="flex gap-3">
                  <div className="flex flex-col items-center">
                    <div className="relative flex h-6 w-6 shrink-0 items-center justify-center">
                      {active && <span className="absolute inline-flex h-full w-full rounded-full bg-neon opacity-25" />}
                      <span className={`relative flex h-6 w-6 items-center justify-center rounded-full border text-[11px] font-bold ${done ? 'border-neon bg-neon text-bg' : active ? 'border-neon bg-neon/15 text-neon' : 'border-brd text-transparent'}`}>
                        {done ? '✓' : i + 1}
                      </span>
                    </div>
                    {!isLast && <div className={`my-0.5 w-px flex-1 ${done ? 'bg-neon/50' : 'bg-brd'}`} />}
                  </div>
                  <div className={isLast ? 'pb-1' : 'pb-4'}>
                    <p className={`text-[13px] font-medium ${done || active ? 'text-text' : 'text-text3'}`}>{step.label}</p>
                    <p className="text-[11px] leading-snug text-text3">{step.sub}</p>
                  </div>
                </div>
              );
            })}
          </div>
        </div>

        {/* Actions */}
        <div className="mb-4 flex w-full max-w-xs gap-2">
          <button
            onClick={() => copyText(connectUri)}
            className="flex-1 rounded-xl border border-brd bg-surface py-2.5 text-[13px] font-medium text-text transition-colors hover:bg-surface2"
          >
            Copy URI
          </button>
          {isMobile() && (
            <button
              onClick={() => { window.location.href = connectUri; }}
              className="flex-1 rounded-xl bg-neon py-2.5 text-[13px] font-medium text-bg transition-colors hover:brightness-110"
            >
              Open Signer
            </button>
          )}
        </div>
        <button onClick={handleCancel} className="w-full py-2.5 text-[13px] text-text3 hover:text-text">
          Cancel
        </button>

        {/* Log panel */}
        {logs.length > 0 && (
          <div className="mt-6 w-full max-w-xs rounded-xl border border-brd bg-black/40 p-3">
            <div ref={logRef} className="max-h-32 overflow-y-auto font-mono text-[10px] leading-relaxed text-text3">
              {logs.map((l, i) => (
                <div key={i} className={l.includes('ERR') ? 'text-red' : ''}>{l}</div>
              ))}
            </div>
          </div>
        )}
      </div>
    );
  }

  // ── START SCREEN ──
  return (
    <div className="flex min-h-dvh flex-col items-center px-5 pb-10 pt-10">
      {/* Ambient glow */}
      <div className="pointer-events-none fixed inset-0 -z-10">
        <div className="absolute left-1/2 top-0 h-[420px] w-[420px] -translate-x-1/2 rounded-full bg-neon/10 blur-[120px]" />
        <div className="absolute bottom-0 left-0 h-80 w-80 rounded-full bg-neon/5 blur-[120px]" />
      </div>

      {/* Header */}
      <div className="mb-8 flex flex-col items-center text-center">
        <div className="relative mb-4 flex h-16 w-16 items-center justify-center rounded-2xl border border-brd bg-surface shadow-lg shadow-neon/5">
          <Shield className="h-7 w-7 text-neon" />
          <div className="absolute inset-0 rounded-2xl ring-1 ring-inset ring-white/5" />
        </div>
        <h1 className="text-xl font-bold tracking-tight text-text">NostrGov</h1>
        <p className="mt-1.5 text-sm text-text3">Decentralized governance on Nostr</p>
      </div>

      {/* Error */}
      {error && (
        <div className="mb-4 w-full max-w-sm rounded-xl border border-red/20 bg-red/10 px-3.5 py-3 text-sm text-red">
          {error}
        </div>
      )}

      <div className="w-full max-w-sm space-y-3">
        {/* NIP-46 — primary */}
        <div className="flex flex-col items-center">
          <div className="mb-3 flex h-24 w-24 items-center justify-center rounded-3xl border border-brd bg-surface/50">
            <QrCode className="h-10 w-10 text-neon" />
          </div>
          <p className="mb-6 max-w-72 text-center text-sm leading-relaxed text-text3">
            Generate a pairing QR and scan it with your signer — no secret keys touch the browser.
          </p>
          <button
            onClick={handleStartConnect}
            className="w-full rounded-2xl bg-neon py-4 text-[15px] font-semibold text-bg transition-all hover:brightness-110 active:scale-[0.98]"
          >
            Connect Signer
          </button>
        </div>

        <div className="flex items-center gap-3 py-2">
          <div className="h-px flex-1 bg-brd" />
          <span className="text-[11px] text-text4">or</span>
          <div className="h-px flex-1 bg-brd" />
        </div>

        {/* NIP-07 */}
        <div className="rounded-[14px] border border-brd bg-surface p-4">
          <div className="mb-2.5 flex items-center gap-2">
            <Fingerprint className="h-4 w-4 text-text2" />
            <span className="text-sm font-medium text-text">Browser Extension</span>
          </div>
          <button
            onClick={loginNip07}
            disabled={loading}
            className="w-full rounded-xl bg-neon/10 py-2.5 text-[13px] font-medium text-neon transition-colors hover:bg-neon/20 disabled:opacity-50"
          >
            {loading ? '…' : 'Login with NIP-07'}
          </button>
        </div>

        {/* nsec */}
        <div className="rounded-[14px] border border-brd bg-surface p-4">
          <div className="mb-2.5 flex items-center gap-2">
            <Key className="h-4 w-4 text-text2" />
            <span className="text-sm font-medium text-text">Private Key</span>
          </div>
          <div className="space-y-2">
            <input
              type="password"
              value={nsecInput}
              onChange={(e) => setNsecInput(e.target.value)}
              placeholder="nsec1..."
              className="h-11 w-full rounded-xl border border-brd bg-surface2 px-4 text-sm text-text placeholder:text-text4 outline-none focus:border-neon/50"
            />
            <button
              onClick={() => nsecInput.trim() && loginNsec(nsecInput.trim())}
              disabled={loading || !nsecInput.trim()}
              className="w-full rounded-xl bg-neon/10 py-2.5 text-[13px] font-medium text-neon transition-colors hover:bg-neon/20 disabled:opacity-50"
            >
              Login
            </button>
          </div>
        </div>

        {/* npub */}
        <div className="rounded-[14px] border border-brd bg-surface p-4">
          <div className="mb-2.5 flex items-center gap-2">
            <Key className="h-4 w-4 text-text2" />
            <span className="text-sm font-medium text-text">Read-only</span>
          </div>
          <div className="space-y-2">
            <input
              type="text"
              value={npubInput}
              onChange={(e) => setNpubInput(e.target.value)}
              placeholder="npub1..."
              className="h-11 w-full rounded-xl border border-brd bg-surface2 px-4 text-sm text-text placeholder:text-text4 outline-none focus:border-neon/50"
            />
            <button
              onClick={() => npubInput.trim() && loginNpub(npubInput.trim())}
              disabled={loading || !npubInput.trim()}
              className="w-full rounded-xl bg-neon/10 py-2.5 text-[13px] font-medium text-neon transition-colors hover:bg-neon/20 disabled:opacity-50"
            >
              View as Guest
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}


