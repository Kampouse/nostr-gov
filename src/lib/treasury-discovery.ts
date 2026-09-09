/**
 * treasury-discovery.ts — find the user's treasuries ON CHAIN.
 *
 * localStorage only ever remembered what THIS browser created; a new
 * device/browser started empty. The chain is the source of truth:
 *
 *   1. scan the account's transaction history (NearBlocks indexer) for
 *      CreateAccount actions on sub-accounts `<name>.<accountId>`
 *   2. probe each candidate with the governance `get_version` view —
 *      only live nostr-gov contracts answer (sub-accounts doing other
 *      things are filtered out)
 *   3. merge with localStorage (which keeps manually-added third-party
 *      treasuries) + DEFAULT_TREASURY
 *
 * Chain wins for ordering (most recent first); localStorage-only entries
 * the chain didn't confirm are still shown — they may be older than the
 * indexer window, or other users' treasuries the user follows.
 */

const NEARBLOCKS = "https://api-testnet.nearblocks.io/v1";
const INDEXER_PAGES = 3; // 25 txns/page — creation is usually recent
const PROBE_TIMEOUT_MS = 4000;

interface IndexerTxn {
  receiver_account_id: string;
  actions?: Array<{ action?: string }>;
}

export async function fetchChainTreasuries(
  accountId: string,
  probeVersion: (contractId: string) => Promise<string | null>,
): Promise<string[]> {
  if (!accountId || !accountId.endsWith(".testnet")) return [];

  // 1. indexer scan → candidate sub-accounts
  const candidates = new Set<string>();
  for (let page = 1; page <= INDEXER_PAGES; page++) {
    let txns: IndexerTxn[] = [];
    try {
      const res = await fetch(
        `${NEARBLOCKS}/account/${accountId}/txns?per_page=25&page=${page}`,
      );
      if (!res.ok) break;
      const body = await res.json();
      txns = body?.txns ?? [];
    } catch {
      break; // indexer down/unreachable — localStorage merge still applies
    }
    if (txns.length === 0) break;
    for (const t of txns) {
      const recv = t.receiver_account_id ?? "";
      const created = (t.actions ?? []).some(
        (a) => (a.action ?? "").toUpperCase() === "CREATE_ACCOUNT",
      );
      if (created && recv.endsWith(`.${accountId}`)) candidates.add(recv);
    }
  }

  // 2. keep only live governance contracts (get_version answers)
  const confirmed: string[] = [];
  await Promise.all(
    [...candidates].map(async (id) => {
      const v = await withTimeout(probeVersion(id), PROBE_TIMEOUT_MS);
      if (v) confirmed.push(id); // any get_version answer = nostr-gov contract
    }),
  );
  return confirmed;
}

function withTimeout<T>(p: Promise<T>, ms: number): Promise<T | null> {
  return Promise.race([
    p.catch(() => null),
    new Promise<null>((r) => setTimeout(() => r(null), ms)),
  ]);
}
