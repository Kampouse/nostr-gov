/// <reference path="../../contract-ts/types/lisp-rlm.d.ts" />
// nostr-gov treasury registry — the global singleton (option B).
//
// Why: NEAR has no sub-account enumeration and plain accounts hold no
// storage, so "which treasuries exist" was localStorage-only and needed an
// off-chain indexer to recover. This contract IS the list: treasuries
// register here at creation (the UI's create batch includes the register
// FunctionCall atomically), and any device recovers an account's
// treasuries with one view call.
//
// Storage layout (string values, nostr-gov convention):
//   o:<owner_account>   → comma-joined treasury ids registered by owner
//   all:<n>             → nth registration (append-only log)
//   n                   → global registration counter (decimal string)
//
// Trust model: public registry, storage staking as the spam gate (same as
// NEAR itself). No admin key — nothing can be censored; owners can remove
// their own entries.

function getStr(k: string) {
  return near.storageGet(k) ?? "";
}
function numStr(k: string) {
  const v = getStr(k);
  return strLength(v) === 0 ? "0" : v;
}
function die(m: string) {
  near.log(m);
  near.panic(m);
}

function ownerKey(owner: string) {
  return strCat("o:", owner);
}

// ── validations ─────────────────────────────────────────────────────────

const TREASURY_CHARS = "abcdefghijklmnopqrstuvwxyz0123456789._-";

function charOk(c: string) {
  return strIndexOf(TREASURY_CHARS, c) !== -1 ? 1 : 0;
}

function treasuryValid(id: string) {
  const n = strLength(id);
  if (n < 8 || n > 64) return 0;                 // shortest: a.b.near
  // testnet or mainnet suffix (sub-account of a user account)
  if (strIndexOf(id, ".testnet") === -1 && strIndexOf(id, ".near") === -1) return 0;
  if (strSlice(id, 0, 1) === "." || strSlice(id, n - 1, n) === ".") return 0;
  let i = 0;
  let ok = 1;
  while (i < n && ok === 1) {
    ok = charOk(strSlice(id, i, i + 1));
    i = i + 1;
  }
  return ok;
}

// whole-segment membership in a comma list ("gov" ≠ "mygov")
function listMember(list: string, item: string) {
  const cur = strCat(",", list, ",");
  return strIndexOf(cur, strCat(",", item, ",")) !== -1 ? 1 : 0;
}

// ── public interface ────────────────────────────────────────────────────

export function get_version() {
  return "1";
}

export function count() {
  return numStr("n");
}

// register { treasury: "gov.alice.testnet", npub?: "64hex" }
// caller (predecessor) becomes the recorded owner; the treasury account
// does not need to exist yet — the UI registers in the same atomic batch
// that creates it.
export function register() {
  const treasury = near.jsonGetStr("treasury") ?? "";
  const npub = near.jsonGetStr("npub") ?? "";
  const owner = near.predecessorAccountId();

  if (treasuryValid(treasury) !== 1) {
    die("ERR_BAD_TREASURY_ID");
  }
  if (strLength(npub) !== 0 && strLength(npub) !== 64) {
    die("ERR_BAD_NPUB");
  }
  // storage staking: owner-list slot + append-only log row (~160B).
  // depositGte takes ONE u128 threshold as (lo64, hi64) — NOT (base,
  // rate): 542 × 2^64 ≈ 0.009998 NEAR (the first draft misread it as a
  // per-byte rate and overflowed the threshold to ~0.5 NEAR).
  // NB: depositGte returns a BOOLEAN-tagged value (emit_tag_bool) — compare
  // with ! directly; `!== 1` is always-true on a bool and inverts the gate.
  if (!near.depositGte(0, 542)) {
    die("ERR_STORAGE_DEPOSIT");
  }

  const cur = getStr(ownerKey(owner));
  if (listMember(cur, treasury) === 1) {
    die("ERR_ALREADY_REGISTERED");
  }
  setOwnerList(owner, strLength(cur) === 0 ? treasury : strCat(cur, ",", treasury));

  const n = numStr("n");
  setStr("all:" + n, strCat(treasury, "|", owner, "|", npub));
  setStr("n", u128Add(n, "1"));
  near.log(strCat("registry: ", treasury, " ← ", owner));
  return 0;
}

// owner-only removal; the append-only log row stays (history), the owner
// list drops the entry (discovery). Staked deposit not refunded — same
// simplicity tradeoff as NEAR itself.
export function unregister() {
  const treasury = near.jsonGetStr("treasury") ?? "";
  const owner = near.predecessorAccountId();
  const cur = getStr(ownerKey(owner));
  if (listMember(cur, treasury) !== 1) {
    die("ERR_NOT_REGISTERED");
  }
  const parts = strSplit(cur, ",");
  let kept = "";
  for (let i = 0; i < parts.length; i++) {
    if (parts[i] !== treasury && strLength(parts[i]) !== 0) {
      kept = strLength(kept) === 0 ? parts[i] : strCat(kept, ",", parts[i]);
    }
  }
  setStr(ownerKey(owner), kept);
  setStr(ownerKey(owner), kept);
  near.log(strCat("registry: removed ", treasury));
  return 0;
}

export function list_by_owner() {
  const owner = near.jsonGetStr("account_id") ?? "";
  return getStr(ownerKey(owner));
}

export function list_all() {
  const total = strToNum(numStr("n"));
  let out = "";
  let i = 0;
  while (i < total) {
    const row = getStr("all:" + toStr(i));
    if (strLength(row) !== 0) {
      out = strLength(out) === 0 ? row : strCat(out, "\n", row);
    }
    i = i + 1;
  }
  return out;
}

function setStr(k: string, v: string) {
  near.storageSet(k, v);
}
function setOwnerList(owner: string, v: string) {
  near.storageSet(ownerKey(owner), v);
}
