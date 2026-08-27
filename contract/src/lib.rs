use near_sdk::borsh::{BorshDeserialize, BorshSerialize};
use near_sdk::collections::{LookupMap, UnorderedSet, Vector};
use near_sdk::json_types::U128;
use near_sdk::{
    env, log, near, near_bindgen, AccountId, Allowance, BorshStorageKey, CurveType,
    NearToken, PanicOnDefault, Promise, PromiseOrValue, PromiseResult, PublicKey,
};

mod ft;
mod message;
mod schnorr;
mod nostr_event;

use base64::Engine;

use message::hex_encode;

// ── Constants ──────────────────────────────────────────────────────────────

/// Maximum proposal expiry: 1 year from now (nanoseconds)
const MAX_EXPIRY_NS: u64 = 365 * 24 * 60 * 60 * 1_000_000_000;
/// Maximum active proposals per intent
const MAX_ACTIVE_PROPOSALS: u32 = 100;
/// Maximum approvers per intent (bitmap is u64)
#[allow(dead_code)]
const MAX_APPROVERS: usize = 64;
/// Storage deposit per wallet (covers wallet + 3 meta-intents + headroom)
const STORAGE_DEPOSIT_YOCTO: u128 = 500_000_000_000_000_000_000_000; // 0.5 NEAR
/// Default execution gas for cross-contract calls (Tgas)
const DEFAULT_EXECUTION_GAS_TGAS: u64 = 50;
/// Maximum execution gas (Tgas)
const MAX_EXECUTION_GAS_TGAS: u64 = 300;
/// Maximum nonce slots a session gas key may reserve (NEP-611 protocol max)
const MAX_SESSION_NONCES: u32 = 1024;
/// Maximum session lifetime: 30 days (nanoseconds)
const MAX_SESSION_DURATION_NS: u64 = 30 * 24 * 60 * 60 * 1_000_000_000;
/// Cumulative funding cap per session gas key (1 NEAR). NEP-611 burns a gas
/// key's remaining balance (up to 1 NEAR) when the key is deleted; above 1
/// NEAR deletion fails outright. Capping funding keeps `revoke_session` able
/// to delete the key on-chain.
const MAX_SESSION_FUND_YOCTO: u128 = 1_000_000_000_000_000_000_000_000;
/// Minimum wallet balance that must remain after funding a session key
const MIN_WALLET_RESERVE_YOCTO: u128 = 100_000_000_000_000_000_000_000; // 0.1 NEAR
/// Methods a session gas key may call on this contract
const SESSION_KEY_METHODS: &str = "submit_action,session_ping";

/// Methods a session gas key may be scoped to (v5). Governance methods
/// (propose/approve/execute/amend/cancel_vote) still verify their own
/// nostr signatures internally, so widening the *transport* scope never
/// widens *authority* — the gas key only pays postage.
const SESSION_KEY_ALLOWED_METHODS: [&str; 9] = [
    "submit_action",
    "session_ping",
    "propose",
    "approve",
    "execute",
    "amend",
    "cancel_vote",
    "approve_with_event",
    "cancel_vote_with_event",
];

// ── Storage Keys ──────────────────────────────────────────────────────────

#[derive(BorshSerialize, BorshStorageKey)]
#[borsh(crate = "near_sdk::borsh")]
enum StorageKey {
    Wallets,
    Intents,
    Proposals,
    Delegations,
    WalletNames,
    Sessions,
    SessionKeys,
}

// ── Types ──────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq, Default)]
#[near(serializers = [borsh, json])]
pub enum IntentType {
    #[default]
    Custom,
    AddIntent,
    RemoveIntent,
    UpdateIntent,
    /// Transfer NEAR or FT tokens to a recipient
    Transfer,
    /// Deposit NEAR into the wallet's internal balance
    Deposit,
    /// Arbitrary cross-contract call
    Call,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
#[near(serializers = [borsh, json])]
pub enum ProposalStatus {
    #[default]
    Active,
    Approved,
    Executed,
    Cancelled,
}

#[derive(Clone, Debug)]
#[near(serializers = [borsh, json])]
pub enum ParamType {
    AccountId,
    U64,
    U128,
    String,
    Bool,
}

#[derive(Clone, Debug)]
#[near(serializers = [borsh, json])]
pub struct ParamDef {
    pub name: String,
    pub param_type: ParamType,
    pub max_value: Option<U128>,
}

#[derive(Clone, Debug)]
#[near(serializers = [borsh, json])]
pub struct Intent {
    pub wallet_name: String,
    pub index: u32,
    pub intent_type: IntentType,
    pub name: String,
    pub template: String,
    pub proposers: Vec<AccountId>,
    pub approvers: Vec<AccountId>,
    /// Nostr npub hex strings (32-byte x-only public keys)
    pub nostr_approvers: Vec<String>,
    /// Total approvals needed (NEAR + nostr combined)
    pub approval_threshold: u16,
    pub cancellation_threshold: u16,
    pub timelock_seconds: u64,
    pub params: Vec<ParamDef>,
    /// Execution gas in teragas (default: 50)
    pub execution_gas_tgas: u64,
    pub active: bool,
    pub active_proposal_count: u32,
}

impl Intent {
    #[allow(dead_code)]
    fn is_proposer(&self, account: &AccountId) -> bool {
        self.proposers.contains(account)
    }

    #[allow(dead_code)]
    fn execution_gas(&self) -> near_sdk::Gas {
        let tgas = if self.execution_gas_tgas == 0 {
            DEFAULT_EXECUTION_GAS_TGAS
        } else {
            self.execution_gas_tgas.min(MAX_EXECUTION_GAS_TGAS)
        };
        near_sdk::Gas::from_tgas(tgas)
    }

    fn render_template(&self, param_values: &serde_json::Value) -> String {
        let mut result = self.template.clone();
        for param in &self.params {
            let placeholder = format!("{{{}}}", param.name);
            let value = match param_values.get(&param.name) {
                Some(v) => match param.param_type {
                    ParamType::AccountId => v.as_str().unwrap_or("unknown").to_string(),
                    ParamType::U64 => v.as_u64().map(|n| n.to_string()).unwrap_or_default(),
                    ParamType::U128 => v
                        .as_str()
                        .map(|s| s.to_string())
                        .or_else(|| match v {
                            serde_json::Value::Number(n) => Some(n.to_string()),
                            _ => None,
                        })
                        .unwrap_or_default(),
                    ParamType::String => v.as_str().unwrap_or("").to_string(),
                    ParamType::Bool => v.as_bool().map(|b| b.to_string()).unwrap_or_default(),
                },
                None => continue,
            };
            // Sanitize: reject message format characters to prevent injection
            assert!(
                !value.contains('|') && !value.contains('\n') && !value.contains('\r'),
                "Param '{}' contains illegal characters",
                param.name
            );
            result = result.replace(&placeholder, &value);
        }
        result
    }
}

#[derive(Clone, Debug)]
#[near(serializers = [borsh, json])]
pub struct Proposal {
    pub id: u64,
    pub wallet_name: String,
    pub intent_index: u32,
    pub proposer: AccountId,
    pub status: ProposalStatus,
    pub proposed_at: u64,
    pub approved_at: u64,
    pub expires_at: u64,
    pub approval_bitmap: u64,
    pub cancellation_bitmap: u64,
    /// Nostr approver bitmaps (same indexing as nostr_approvers)
    pub nostr_approval_bitmap: u64,
    pub nostr_cancellation_bitmap: u64,
    pub param_values: String,
    pub message: String,
    /// SHA-256 of the intent's params schema at proposal time.
    /// Execution fails if the schema changed after proposal.
    pub intent_params_hash: String,
}

impl Proposal {
    fn approval_count(&self) -> u32 {
        self.approval_bitmap.count_ones() + self.nostr_approval_bitmap.count_ones()
    }

    fn cancellation_count(&self) -> u32 {
        self.cancellation_bitmap.count_ones() + self.nostr_cancellation_bitmap.count_ones()
    }

    #[allow(dead_code)]
    fn has_approved(&self, idx: usize) -> bool {
        (self.approval_bitmap & (1u64 << idx)) != 0
    }

    fn has_nostr_approved(&self, idx: usize) -> bool {
        (self.nostr_approval_bitmap & (1u64 << idx)) != 0
    }

    #[allow(dead_code)]
    fn set_approval(&mut self, idx: usize) {
        let mask = 1u64 << idx;
        self.cancellation_bitmap &= !mask;
        self.approval_bitmap |= mask;
    }

    fn set_nostr_approval(&mut self, idx: usize) {
        let mask = 1u64 << idx;
        self.nostr_cancellation_bitmap &= !mask;
        self.nostr_approval_bitmap |= mask;
    }

    #[allow(dead_code)]
    fn set_cancellation(&mut self, idx: usize) {
        let mask = 1u64 << idx;
        self.approval_bitmap &= !mask;
        self.cancellation_bitmap |= mask;
    }

    fn set_nostr_cancellation(&mut self, idx: usize) {
        let mask = 1u64 << idx;
        self.nostr_approval_bitmap &= !mask;
        self.nostr_cancellation_bitmap |= mask;
    }

    fn reset_votes(&mut self) {
        self.approval_bitmap = 0;
        self.cancellation_bitmap = 0;
        self.nostr_approval_bitmap = 0;
        self.nostr_cancellation_bitmap = 0;
        self.approved_at = 0;
    }
}

// ── Session Keys (v4 — NEP-611 gas keys) ───────────────────────────────────

/// Metadata for a registered session gas key.
/// The actual key (balance, nonces, allowed methods) lives in the account's
/// access-key state; this is the contract's registry of which sessions exist.
#[derive(Clone, Debug)]
#[near(serializers = [borsh, json])]
pub struct SessionMeta {
    /// ed25519 public key, 64 hex chars, no "ed25519:" prefix
    pub public_key: String,
    /// Wallet the session may operate on
    pub wallet: String,
    /// Creation time (nanoseconds)
    pub created_at: u64,
    /// Expiry time (nanoseconds). Session is refused at or after this time.
    pub expires_at: u64,
    /// Cumulative yoctoNEAR moved into the gas key via transfer_to_gas_key
    pub funded_yocto: u128,
    /// Optional human label
    pub label: Option<String>,
}

impl SessionMeta {
    fn is_active(&self, now_ns: u64) -> bool {
        now_ns < self.expires_at
    }
}

/// Normalize a public key string to 64 lowercase hex chars (no prefix).
fn normalize_pk_hex(public_key: &str) -> String {
    let pk = public_key.trim().to_lowercase();
    let pk = pk.strip_prefix("ed25519:").unwrap_or(&pk);
    assert!(pk.len() == 64, "ERR_INVALID_PK_LEN: expected 64 hex chars");
    assert!(pk.chars().all(|c| c.is_ascii_hexdigit()), "ERR_INVALID_PK_HEX");
    pk.to_string()
}

/// Rebuild a near_sdk::PublicKey from a 64-char hex ed25519 key.
fn parse_ed25519_pk(pk_hex: &str) -> PublicKey {
    let bytes = message::hex_decode(pk_hex);
    PublicKey::from_parts(CurveType::ED25519, bytes)
        .unwrap_or_else(|_| env::panic_str("ERR_INVALID_PK"))
}

// ── Wallet Migration ──────────────────────────────────────────────────────

/// Old wallet format (V1) — no spending limits or relayer config.
/// Used only for Borsh deserialization of legacy state.
#[derive(BorshSerialize, BorshDeserialize)]
#[borsh(crate = "near_sdk::borsh")]
struct WalletV1 {
    name: String,
    owner: AccountId,
    proposal_index: u64,
    intent_index: u32,
    created_at: u64,
    storage_deposit: u128,
    storage_used: u64,
    allowed_tokens: Vec<AccountId>,
    ft_token_count: u32,
}

impl From<WalletV1> for Wallet {
    fn from(v1: WalletV1) -> Self {
        Wallet {
            name: v1.name,
            owner: v1.owner,
            proposal_index: v1.proposal_index,
            intent_index: v1.intent_index,
            created_at: v1.created_at,
            storage_deposit: v1.storage_deposit,
            storage_used: v1.storage_used,
            allowed_tokens: v1.allowed_tokens,
            ft_token_count: v1.ft_token_count,
            // New fields get safe defaults
            call_allowed_receivers: Vec::new(),
            call_max_deposit: 0,
            daily_spend_limit: 0,
            daily_spend_reset_at: v1.created_at,
            daily_spend_used: 0,
            relayer_fee: 0,
            allowed_relayers: Vec::new(),
        }
    }
}

#[derive(Clone, Debug)]
#[near(serializers = [borsh, json])]
pub struct Wallet {
    pub name: String,
    pub owner: AccountId,
    pub proposal_index: u64,
    pub intent_index: u32,
    pub created_at: u64,
    /// Amount of NEAR deposited for storage (yoctoNEAR)
    pub storage_deposit: u128,
    /// Actual storage used (bytes), tracked for accurate refunds
    pub storage_used: u64,
    /// Tokens allowed to be received via ft_on_transfer.
    /// Empty = accept all (open), non-empty = allowlist only.
    pub allowed_tokens: Vec<AccountId>,
    /// Number of unique FT tokens tracked (for storage accounting)
    pub ft_token_count: u32,
    /// Allowed receiver contracts for Call intents (empty = all allowed)
    pub call_allowed_receivers: Vec<AccountId>,
    /// Max deposit per Call intent in yoctoNEAR (0 = no limit)
    pub call_max_deposit: u128,
    /// Max total spend per day in yoctoNEAR (0 = no limit)
    pub daily_spend_limit: u128,
    /// Timestamp of last daily spend reset (nanoseconds)
    pub daily_spend_reset_at: u64,
    /// Amount spent today in yoctoNEAR
    pub daily_spend_used: u128,
    /// Relayer fee in yoctoNEAR per execution (0 = free)
    pub relayer_fee: u128,
    /// Optional relayer allowlist (empty = anyone can relay)
    pub allowed_relayers: Vec<AccountId>,
}

impl Wallet {
    /// Check and enforce daily spend limit. Returns true if reset happened.
    fn enforce_spend_limit(&mut self, now_ns: u64) {
        if self.daily_spend_limit == 0 { return; }
        // Reset if more than 24h have passed
        let one_day_ns: u64 = 24 * 60 * 60 * 1_000_000_000;
        if now_ns >= self.daily_spend_reset_at + one_day_ns {
            self.daily_spend_used = 0;
            self.daily_spend_reset_at = now_ns;
        }
    }

    /// Track spending and enforce limits. Panics if over limit.
    fn track_spend(&mut self, amount: u128) {
        if self.daily_spend_limit == 0 { return; }
        let new_total = self.daily_spend_used + amount;
        assert!(
            new_total <= self.daily_spend_limit,
            "ERR_DAILY_SPEND_LIMIT: used {} + {} > limit {}",
            self.daily_spend_used, amount, self.daily_spend_limit
        );
        self.daily_spend_used = new_total;
    }

    /// Check if a receiver is allowed for Call intents
    fn is_call_receiver_allowed(&self, receiver: &AccountId) -> bool {
        self.call_allowed_receivers.is_empty() || self.call_allowed_receivers.contains(receiver)
    }

    /// Check if a relayer is allowed
    #[allow(dead_code)]
    fn is_relayer_allowed(&self, relayer: &AccountId) -> bool {
        self.allowed_relayers.is_empty() || self.allowed_relayers.contains(relayer)
    }
}

// ── Composite keys ─────────────────────────────────────────────────────────

fn intent_key(wallet: &str, index: u32) -> String {
    format!("{}:i:{}", wallet, index)
}

fn proposal_key(wallet: &str, id: u64) -> String {
    format!("{}:p:{}", wallet, id)
}

fn delegation_key(wallet: &str, intent_index: u32, approver_index: usize) -> String {
    format!("{}:d:{}:{}", wallet, intent_index, approver_index)
}

// ── Helpers ────────────────────────────────────────────────────────────────

/// SHA-256 of Borsh-serialized params schema.
fn hash_params(params: &[ParamDef]) -> String {
    let mut data = Vec::new();
    near_sdk::borsh::BorshSerialize::serialize(params, &mut data)
        .unwrap_or_else(|_| env::panic_str("Failed to serialize params"));
    let hash = env::sha256(&data);
    hash.iter().map(|b| format!("{:02x}", b)).collect()
}

/// Panic if the call comes from a contract (not a direct user transaction).
#[allow(dead_code)]
fn assert_direct_call() {
    assert_eq!(
        env::signer_account_id(),
        env::predecessor_account_id(),
        "ERR_DIRECT_CALL_REQUIRED"
    );
}

/// Get the hex representation of the signer's ed25519 public key (32 bytes, no prefix).
#[allow(dead_code)]
fn signer_pk_hex() -> String {
    let pk = env::signer_account_pk();
    let bytes = pk.into_bytes();
    // near-sdk PublicKey: 1-byte curve type prefix (0x00 = ed25519) + 32 bytes key
    let raw = if bytes.len() == 33 { &bytes[1..] } else { &bytes[..] };
    hex_encode(raw)
}

/// Build a JSON string safely using serde_json (no string formatting for JSON).
fn safe_json_ft_transfer(recipient: &str, amount: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "receiver_id": recipient,
        "amount": amount,
        "msg": ""
    }))
    .unwrap_or_else(|_| env::panic_str("ERR_JSON_SERIALIZE"))
}

// ── Contract ───────────────────────────────────────────────────────────────

#[near(contract_state)]
#[derive(PanicOnDefault)]
pub struct Contract {
    /// Contract version for migration tracking.
    version: u32,
    /// Owner nostr npubs (hex). Any owner key can authorize owner actions.
    owner_npubs: Vec<String>,
    /// Optional guardian npub — may ONLY pause the contract.
    guardian_npub: Option<String>,
    wallets: LookupMap<String, Wallet>,
    intents: LookupMap<String, Intent>,
    proposals: LookupMap<String, Proposal>,
    delegations: LookupMap<String, AccountId>,
    event_nonce: u64,
    /// Window base for owner nonce replay protection.
    owner_nonce: u64,
    /// Bitmap of consumed nonces within [owner_nonce, owner_nonce+64).
    owner_nonce_bitmap: u64,
    /// Ordered list of wallet names for enumeration.
    wallet_names: Vector<String>,
    /// Emergency pause flag — when true, only unpause() works.
    paused: bool,
    /// Reentrancy guard — true while executing a cross-contract call.
    locked: bool,
    /// v4: registered session gas keys (pubkey hex -> meta)
    sessions: LookupMap<String, SessionMeta>,
    /// v4: enumerable set of session pubkey hexes
    session_keys: UnorderedSet<String>,
}

/// v3 contract state (before session keys) — used only for state migration.
#[derive(BorshDeserialize, BorshSerialize)]
#[borsh(crate = "near_sdk::borsh")]
struct ContractV3 {
    version: u32,
    owner_npubs: Vec<String>,
    guardian_npub: Option<String>,
    wallets: LookupMap<String, Wallet>,
    intents: LookupMap<String, Intent>,
    proposals: LookupMap<String, Proposal>,
    delegations: LookupMap<String, AccountId>,
    event_nonce: u64,
    owner_nonce: u64,
    owner_nonce_bitmap: u64,
    wallet_names: Vector<String>,
    paused: bool,
    locked: bool,
}

impl From<ContractV3> for Contract {
    fn from(old: ContractV3) -> Self {
        Contract {
            version: 5,
            owner_npubs: old.owner_npubs,
            guardian_npub: old.guardian_npub,
            wallets: old.wallets,
            intents: old.intents,
            proposals: old.proposals,
            delegations: old.delegations,
            event_nonce: old.event_nonce,
            owner_nonce: old.owner_nonce,
            owner_nonce_bitmap: old.owner_nonce_bitmap,
            wallet_names: old.wallet_names,
            paused: old.paused,
            locked: old.locked,
            sessions: LookupMap::new(StorageKey::Sessions),
            session_keys: UnorderedSet::new(StorageKey::SessionKeys),
        }
    }
}

#[near_bindgen]
impl Contract {
    /// State migration: called on deserialization to handle new fields.
    #[allow(dead_code)]
    fn init_state(&mut self) {
        if self.paused {} // ensure field exists on old state
        if self.version == 0 { self.version = 2; } // migration from v0/v1
        if self.version == 3 { self.version = 4; } // v3 state under v4 code (migrate() path fallback)
        if self.version == 4 { self.version = 5; } // v4 state under v5 code (add_session_key methods param)
    }

    /// Assert contract is not paused.
    fn assert_not_paused(&self) {
        assert!(!self.paused, "ERR_CONTRACT_PAUSED");
    }

    /// Assert not in a cross-contract call (reentrancy guard).
    fn assert_not_locked(&self) {
        assert!(!self.locked, "ERR_REENTRANCY");
    }

    /// Emergency pause — blocks all state-changing calls except unpause.
    /// Authorized by ANY owner npub OR the guardian npub (guardian can only pause).
    pub fn pause(&mut self, signature: String, expires_at: u64) {
        assert!(expires_at > env::block_timestamp(), "ERR_SIG_EXPIRED");
        let msg = format!(
            "expires {}.000000000: pause | contract: {}",
            expires_at, env::current_account_id()
        );
        let is_owner = self.owner_npubs.iter()
            .any(|pk| message::try_schnorr_verify(pk, &signature, &msg));
        let is_guardian = self.guardian_npub.as_deref()
            .map_or(false, |g| message::try_schnorr_verify(g, &signature, &msg));
        assert!(is_owner || is_guardian, "ERR_NOT_AUTHORIZED_TO_PAUSE");
        self.paused = true;
        self.emit_event("contract_paused", serde_json::json!({}));
        log!("Contract paused");
    }

    /// Unpause — restores normal operation.
    pub fn unpause(&mut self, signature: String, expires_at: u64, nonce: u64) {
        self.verify_owner("unpause", &signature, expires_at, nonce);
        self.paused = false;
        self.emit_event("contract_unpaused", serde_json::json!({}));
        log!("Contract unpaused");
    }

    /// Check if contract is paused.
    pub fn is_paused(&self) -> bool {
        self.paused
    }

    /// Get contract version.
    pub fn get_version(&self) -> u32 {
        self.version
    }

    // ── Session Keys (v4 — NEP-611 gas keys) ────────────────────────

    /// Resolve the calling signer's public key to a registered, unexpired
    /// session. Gas-key transactions are signed by a key ON the contract
    /// account, so `env::signer_account_pk()` identifies the session.
    fn require_session_caller(&self) -> (String, SessionMeta) {
        let pk = signer_pk_hex();
        let meta = self
            .sessions
            .get(&pk)
            .unwrap_or_else(|| env::panic_str("ERR_NOT_SESSION_KEY"));
        assert!(
            meta.is_active(env::block_timestamp()),
            "ERR_SESSION_EXPIRED"
        );
        (pk, meta)
    }

    /// Register a session gas key for a wallet. The key can then pay for and
    /// submit `submit_action` / `session_ping` calls directly — no relayer
    /// needed. Requires an owner nostr signature over
    /// `expires {sig_expiry}.000000000: add_session_key:{pk_hex} | nonce: {n} | contract: {id}`.
    ///
    /// The initial gas is debited from the wallet's internal NEAR balance and
    /// moved into the gas key's prepaid balance via `transfer_to_gas_key`.
    pub fn add_session_key(
        &mut self,
        public_key: String,
        num_nonces: u32,
        expires_at: u64,
        wallet: String,
        label: Option<String>,
        initial_gas: U128,
        // v5: optional method scope for the gas key. Defaults to the
        // v4 transport scope (submit_action,session_ping). Each entry
        // must be in SESSION_KEY_ALLOWED_METHODS.
        methods: Option<Vec<String>>,
        nonce: u64,
        expires_at_sig: u64,
        signature: String,
    ) {
        self.assert_not_paused();
        let pk = normalize_pk_hex(&public_key);
        self.verify_owner(
            &format!("add_session_key:{}", pk),
            &signature,
            expires_at_sig,
            nonce,
        );

        assert!(self.sessions.get(&pk).is_none(), "ERR_SESSION_EXISTS");
        assert!(
            self.wallet_get_readonly(&wallet).is_some(),
            "ERR_WALLET_NOT_FOUND"
        );

        let now = env::block_timestamp();
        assert!(expires_at > now, "ERR_SESSION_EXPIRED");
        let expires_at = expires_at.min(now + MAX_SESSION_DURATION_NS);
        let nn = num_nonces.clamp(1, MAX_SESSION_NONCES);
        let gas = initial_gas.0;
        assert!(
            gas <= MAX_SESSION_FUND_YOCTO,
            "ERR_SESSION_FUND_CAP: cumulative funding max 1 NEAR"
        );

        if gas > 0 {
            let bal = self.get_wallet_near_balance(wallet.clone()).0;
            assert!(
                bal >= gas + MIN_WALLET_RESERVE_YOCTO,
                "ERR_SESSION_INSUFFICIENT_BALANCE: wallet needs {} + 0.1 NEAR reserve",
                gas
            );
            self.debit_near(&wallet, gas);
        }

        let meta = SessionMeta {
            public_key: pk.clone(),
            wallet: wallet.clone(),
            created_at: now,
            expires_at,
            funded_yocto: gas,
            label: label.clone(),
        };
        self.sessions.insert(&pk, &meta);
        self.session_keys.insert(&pk);

        let self_id = env::current_account_id();
        let parsed = parse_ed25519_pk(&pk);
        // v5: resolve method scope (validated) or fall back to v4 default
        let methods_csv = match &methods {
            Some(list) => {
                assert!(!list.is_empty(), "ERR_SESSION_METHODS_EMPTY");
                let mut seen = std::collections::HashSet::new();
                for m in list {
                    assert!(
                        SESSION_KEY_ALLOWED_METHODS.contains(&m.as_str()),
                        "ERR_SESSION_METHOD_NOT_ALLOWED: {} (allowed: {})",
                        m,
                        SESSION_KEY_ALLOWED_METHODS.join(", ")
                    );
                    assert!(seen.insert(m.clone()), "ERR_SESSION_METHOD_DUPLICATE: {}", m);
                }
                list.join(",")
            }
            None => SESSION_KEY_METHODS.to_string(),
        };
        let mut batch = Promise::new(self_id.clone()).add_gas_key_allowance_function_call(
            parsed.clone(),
            nn,
            Allowance::Unlimited,
            self_id.clone(),
            methods_csv.clone(),
        );
        if gas > 0 {
            batch = batch.transfer_to_gas_key(parsed, NearToken::from_yoctonear(gas));
        }
        let _ = batch; // promise scheduled; state committed regardless of receipt outcome

        self.emit_event("session_key_added", serde_json::json!({
            "public_key": pk,
            "wallet": wallet,
            "expires_at": expires_at,
            "num_nonces": nn,
            "initial_gas": gas.to_string(),
            "methods": methods_csv,
        }));
        log!(
            "Session key {}… added for wallet '{}' ({} nonces, {} yoctoNEAR gas)",
            &pk[..16],
            wallet,
            nn,
            gas
        );
    }

    /// Top up a session gas key's prepaid balance from the wallet's internal
    /// NEAR balance. Owner-signed over
    /// `expires {sig_expiry}.000000000: refresh_session:{pk_hex}:{amount_yocto} | nonce: {n} | contract: {id}`.
    pub fn refresh_session(
        &mut self,
        public_key: String,
        amount: U128,
        nonce: u64,
        expires_at_sig: u64,
        signature: String,
    ) {
        self.assert_not_paused();
        let pk = normalize_pk_hex(&public_key);
        self.verify_owner(
            &format!("refresh_session:{}:{}", pk, amount.0),
            &signature,
            expires_at_sig,
            nonce,
        );

        let mut meta = self
            .sessions
            .get(&pk)
            .unwrap_or_else(|| env::panic_str("ERR_SESSION_NOT_FOUND"));
        assert!(
            meta.is_active(env::block_timestamp()),
            "ERR_SESSION_EXPIRED"
        );
        let amt = amount.0;
        let new_funded = meta.funded_yocto + amt;
        assert!(
            new_funded <= MAX_SESSION_FUND_YOCTO,
            "ERR_SESSION_FUND_CAP: cumulative funding max 1 NEAR"
        );
        assert!(amt > 0, "ERR_ZERO_AMOUNT");

        let bal = self.get_wallet_near_balance(meta.wallet.clone()).0;
        assert!(
            bal >= amt + MIN_WALLET_RESERVE_YOCTO,
            "ERR_SESSION_INSUFFICIENT_BALANCE"
        );
        self.debit_near(&meta.wallet, amt);

        meta.funded_yocto = new_funded;
        self.sessions.insert(&pk, &meta);

        let self_id = env::current_account_id();
        Promise::new(self_id).transfer_to_gas_key(
            parse_ed25519_pk(&pk),
            NearToken::from_yoctonear(amt),
        );

        self.emit_event("session_key_refreshed", serde_json::json!({
            "public_key": pk,
            "wallet": meta.wallet,
            "amount": amt.to_string(),
            "funded_total": meta.funded_yocto.to_string(),
        }));
        log!("Session key {}… topped up with {} yoctoNEAR", &pk[..16], amt);
    }

    /// Revoke a session: removes the contract's SessionMeta and schedules a
    /// `delete_key` on the gas key. NEP-611 burns any remaining gas-key
    /// balance ≤ 1 NEAR on deletion (funding is capped at 1 NEAR for this
    /// reason). To salvage unspent NEAR instead, first run
    /// `near account withdraw-from-gas-key …` from the owner CLI.
    /// Owner-signed over
    /// `expires {sig_expiry}.000000000: revoke_session:{pk_hex} | nonce: {n} | contract: {id}`.
    pub fn revoke_session(
        &mut self,
        public_key: String,
        nonce: u64,
        expires_at_sig: u64,
        signature: String,
    ) {
        self.verify_owner(
            &format!("revoke_session:{}", normalize_pk_hex(&public_key)),
            &signature,
            expires_at_sig,
            nonce,
        );
        let pk = normalize_pk_hex(&public_key);
        let meta = self
            .sessions
            .get(&pk)
            .unwrap_or_else(|| env::panic_str("ERR_SESSION_NOT_FOUND"));

        self.sessions.remove(&pk);
        self.session_keys.remove(&pk);

        Promise::new(env::current_account_id()).delete_key(parse_ed25519_pk(&pk));

        self.emit_event("session_key_revoked", serde_json::json!({
            "public_key": pk,
            "wallet": meta.wallet,
        }));
        log!(
            "Session key {}… revoked (remaining gas-key balance is burned per NEP-611)",
            &pk[..16]
        );
    }

    /// Relayer-free entry point that ONLY a registered session gas key may
    /// call (the gas key's prepaid balance pays for the transaction). The
    /// inner action still requires the owner's nostr signature, using the
    /// exact v3 message formats, so the owner authorizes every action while
    /// the session key only supplies gas. Available inner actions:
    /// `propose`, `amend`, `execute`, `quick`.
    ///
    /// NOTE: gas keys cannot attach deposits, so Deposit-type executions
    /// through this path credit 0 NEAR — use the relayer path for deposits.
    pub fn submit_action(
        &mut self,
        action: String,
        wallet_name: String,
        intent_index: u32,
        proposal_id: u64,
        param_values: String,
        expires_at: u64,
        nonce: u64,
        signature: String,
    ) {
        self.assert_not_paused();
        self.assert_not_locked();
        let (pk, meta) = self.require_session_caller();
        assert_eq!(meta.wallet, wallet_name, "ERR_SESSION_WALLET_MISMATCH");
        log!("submit_action '{}' via session key {}…", action, &pk[..16]);

        match action.as_str() {
            "propose" => {
                let pid = self
                    .wallet_get_readonly(&wallet_name)
                    .expect("ERR_WALLET_NOT_FOUND")
                    .proposal_index;
                self.verify_owner(
                    &format!("propose:{}:{}", wallet_name, pid),
                    &signature,
                    expires_at,
                    nonce,
                );
                self.propose_inner(wallet_name.clone(), intent_index, param_values, expires_at);
            }
            "amend" => {
                self.verify_owner(
                    &format!("amend:{}:{}", wallet_name, proposal_id),
                    &signature,
                    expires_at,
                    nonce,
                );
                self.amend_inner(wallet_name.clone(), proposal_id, param_values, expires_at);
            }
            "execute" => {
                self.verify_owner(
                    &format!("execute:{}:{}", wallet_name, proposal_id),
                    &signature,
                    expires_at,
                    nonce,
                );
                self.execute_proposal(&wallet_name, proposal_id);
            }
            "quick" => {
                let action_str = format!(
                    "quick:{}:{}:{}",
                    wallet_name,
                    intent_index,
                    &param_values.chars().take(64).collect::<String>()
                );
                self.verify_owner(&action_str, &signature, expires_at, nonce);
                self.quick_execute_inner(wallet_name.clone(), intent_index, param_values, expires_at, true);
            }
            _ => env::panic_str("ERR_INVALID_ACTION: expected propose|amend|execute|quick"),
        }
    }

    /// Health check / gas-key probe. Only a registered, unexpired session key
    /// may call it; the gas key's balance pays the (small) fee.
    pub fn session_ping(&mut self) -> String {
        self.assert_not_paused();
        let (pk, meta) = self.require_session_caller();
        format!("pong:{}:{}", meta.wallet, &pk[..16])
    }

    /// List session metadata for a wallet. (Gas-key balances are not visible
    /// on-chain from the contract — query `view_access_key` via RPC for those.)
    pub fn list_sessions(&self, wallet: String) -> Vec<SessionMeta> {
        self.session_keys
            .iter()
            .filter_map(|pk| self.sessions.get(&pk))
            .filter(|m| m.wallet == wallet)
            .collect()
    }

    /// Get one session's metadata by public key (hex or "ed25519:…" form).
    pub fn get_session(&self, public_key: String) -> Option<SessionMeta> {
        let pk = normalize_pk_hex(&public_key);
        self.sessions.get(&pk)
    }

    /// Total number of registered sessions.
    pub fn get_session_count(&self) -> u64 {
        self.session_keys.len()
    }

    // ── Wallet Storage Helpers (with migration) ──────────────────────

    /// Get a wallet, automatically migrating from old format if needed.
    /// Writes back the migrated version so future reads are V2-native.
    fn wallet_get(&mut self, name: &str) -> Option<Wallet> {
        let key = name.to_string();
        // Try V2 (current) deserialization first
        if let Some(wallet) = self.wallets.get(&key) {
            return Some(wallet);
        }
        // V2 failed — try V1 migration via raw storage
        let storage_key = {
            let mut k = Vec::new();
            near_sdk::borsh::BorshSerialize::serialize(&StorageKey::Wallets, &mut k).unwrap();
            near_sdk::borsh::BorshSerialize::serialize(&key, &mut k).unwrap();
            k
        };
        if let Some(bytes) = env::storage_read(&storage_key) {
            if let Ok(v1) = WalletV1::try_from_slice(&bytes) {
                let wallet: Wallet = v1.into();
                self.wallets.insert(&key, &wallet);
                log!("Migrated wallet '{}' from V1 to V2", name);
                return Some(wallet);
            }
        }
        None
    }

    fn wallet_get_readonly(&self, name: &str) -> Option<Wallet> {
        self.wallets.get(&name.to_string())
    }

    fn wallet_insert(&mut self, name: &str, wallet: &Wallet) {
        self.wallets.insert(&name.to_string(), wallet);
    }

    fn wallet_remove(&mut self, name: &str) -> Option<Wallet> {
        self.wallets.remove(&name.to_string())
    }

    /// Initialize with the nostr npub of the owner.
    /// The owner controls everything — create wallets, add intents, propose.
    #[init]
    pub fn new(owner_npubs: Vec<String>) -> Self {
        assert!(!owner_npubs.is_empty(), "ERR_EMPTY_OWNERS");
        assert!(owner_npubs.len() <= 8, "ERR_TOO_MANY_OWNERS: max 8");
        for np in &owner_npubs {
            assert!(np.len() == 64, "ERR_INVALID_NPUB_LEN: expected 64 hex chars");
            assert!(np.chars().all(|c| c.is_ascii_hexdigit()), "ERR_INVALID_NPUB_HEX");
        }
        Self {
            version: 5,
            owner_npubs,
            guardian_npub: None,
            wallets: LookupMap::new(StorageKey::Wallets),
            intents: LookupMap::new(StorageKey::Intents),
            proposals: LookupMap::new(StorageKey::Proposals),
            delegations: LookupMap::new(StorageKey::Delegations),
            event_nonce: 0,
            owner_nonce: 0,
            owner_nonce_bitmap: 0,
            wallet_names: Vector::new(StorageKey::WalletNames),
            paused: false,
            locked: false,
            sessions: LookupMap::new(StorageKey::Sessions),
            session_keys: UnorderedSet::new(StorageKey::SessionKeys),
        }
    }
    /// Force-reinit: ignores existing state, creates fresh v5 contract.
    #[init(ignore_state)]
    pub fn reinit(owner_npubs: Vec<String>) -> Self {
        assert!(!owner_npubs.is_empty(), "ERR_EMPTY_OWNERS");
        assert!(owner_npubs.len() <= 8, "ERR_TOO_MANY_OWNERS: max 8");
        for np in &owner_npubs {
            assert!(np.len() == 64, "ERR_INVALID_NPUB_LEN");
            assert!(np.chars().all(|c| c.is_ascii_hexdigit()), "ERR_INVALID_NPUB_HEX");
        }
        Self {
            version: 5,
            owner_npubs, guardian_npub: None,
            wallets: LookupMap::new(StorageKey::Wallets),
            intents: LookupMap::new(StorageKey::Intents),
            proposals: LookupMap::new(StorageKey::Proposals),
            delegations: LookupMap::new(StorageKey::Delegations),
            event_nonce: 0, owner_nonce: 0, owner_nonce_bitmap: 0,
            wallet_names: Vector::new(StorageKey::WalletNames),
            paused: false, locked: false,
            sessions: LookupMap::new(StorageKey::Sessions),
            session_keys: UnorderedSet::new(StorageKey::SessionKeys),
        }
    }

    /// Migrate v3 state to v5 (adds empty session registries).
    /// Call once after deploying v5 code over a v3 contract.
    ///
    /// Guards (Tier-1 hardening):
    /// - Deserialization guard: a current v5 state fails to deserialize as
    ///   `ContractV3` (trailing session registry bytes) → `ERR_NO_PREVIOUS_STATE`.
    /// - One-shot guard: `assert_eq!(old.version, 3)` — if a future layout ever
    ///   deserializes prefix-compatible, re-running migrate() aborts instead of
    ///   silently rebuilding state (which would wipe all registered session keys).
    #[init(ignore_state)]
    pub fn migrate() -> Self {
        let old: ContractV3 = env::state_read().expect("ERR_NO_PREVIOUS_STATE");
        assert_eq!(old.version, 3, "ERR_NOT_V3_STATE");
        log!("Migrated contract state v{} -> v5", old.version);
        old.into()
    }

    /// Verify the caller is an owner via schnorr signature over
    /// `expires {ts}.000000000: {action} | nonce: {n} | contract: {account_id}`.
    /// Binds the signature to THIS contract account (anti cross-contract replay)
    /// and to a client-chosen nonce inside a 64-slot sliding window (safe for
    /// concurrent relayers).
    fn verify_owner(&mut self, action: &str, signature: &str, expires_at: u64, nonce: u64) {
        assert!(expires_at > env::block_timestamp(), "ERR_SIG_EXPIRED");
        let msg = format!(
            "expires {}.000000000: {} | nonce: {} | contract: {}",
            expires_at, action, nonce, env::current_account_id()
        );
        let valid = self.owner_npubs.iter()
            .any(|pk| message::try_schnorr_verify(pk, signature, &msg));
        assert!(valid, "ERR_INVALID_OWNER_SIGNATURE");
        self.consume_nonce(nonce);
    }

    /// Sliding 64-slot nonce window: any unused nonce in
    /// [owner_nonce, owner_nonce+64) is accepted; the window slides forward
    /// as low nonces are consumed. Prevents both replay and concurrent-tx races.
    fn consume_nonce(&mut self, nonce: u64) {
        assert!(nonce >= self.owner_nonce, "ERR_NONCE_TOO_LOW");
        assert!(nonce < self.owner_nonce + 64, "ERR_NONCE_WINDOW_EXCEEDED");
        let bit = 1u64 << (nonce - self.owner_nonce);
        assert!(self.owner_nonce_bitmap & bit == 0, "ERR_NONCE_ALREADY_USED");
        self.owner_nonce_bitmap |= bit;
        while self.owner_nonce_bitmap & 1 != 0 {
            self.owner_nonce += 1;
            self.owner_nonce_bitmap >>= 1;
        }
    }

    // ── Wallet Management ──────────────────────────────────────────────

    #[payable]
    pub fn create_wallet(&mut self, name: String, signature: String, expires_at: u64, nonce: u64) {
        self.assert_not_paused();
        self.verify_owner(&format!("create_wallet:{}", name), &signature, expires_at, nonce);
        let deposit = env::attached_deposit();
        assert!(
            deposit.as_yoctonear() >= STORAGE_DEPOSIT_YOCTO,
            "ERR_STORAGE_DEPOSIT: need {} yoctoNEAR, got {}",
            STORAGE_DEPOSIT_YOCTO,
            deposit.as_yoctonear()
        );

        assert!(self.wallet_get(&name).is_none(), "ERR_WALLET_EXISTS");
        assert!(!name.is_empty(), "ERR_NAME_EMPTY");
        assert!(name.len() <= 64, "ERR_NAME_TOO_LONG");
        assert!(
            name.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_'),
            "ERR_NAME_INVALID_CHARS"
        );

        let owner_display = self.owner_npubs.first().cloned().unwrap_or_default();
        let initial_storage = env::storage_usage();
        let wallet = Wallet {
            name: name.clone(),
            owner: env::predecessor_account_id(),
            proposal_index: 0,
            intent_index: 3,
            created_at: env::block_timestamp(),
            storage_deposit: deposit.as_yoctonear(),
            storage_used: 0,
            allowed_tokens: Vec::new(),
            ft_token_count: 0,
            call_allowed_receivers: Vec::new(),
            call_max_deposit: 0,
            daily_spend_limit: 0,
            daily_spend_reset_at: env::block_timestamp(),
            daily_spend_used: 0,
            relayer_fee: 0,
            allowed_relayers: Vec::new(),
        };
        self.wallet_insert(&name, &wallet);
        self.wallet_names.push(&name);
        self.create_meta_intents(&name, &env::predecessor_account_id());
        let storage_used = env::storage_usage() - initial_storage;

        // Update with actual storage usage
        let mut w = self.wallet_get(&name).unwrap();
        w.storage_used = storage_used;
        self.wallet_insert(&name, &w);

        self.emit_event("wallet_created", serde_json::json!({
            "wallet": name,
            "owner_npub": owner_display,
            "deposit": deposit.as_yoctonear().to_string(),
            "storage_used": storage_used,
        }));

        log!("Wallet '{}' created ({} bytes storage)", name, storage_used);
    }

    /// Test: verify a nostr schnorr signature. Returns true if valid.
    pub fn test_verify_nostr(
        &self,
        message: String,
        pubkey_hex: String,
        signature: String,
    ) -> bool {
        message::verify_schnorr_signature(&pubkey_hex, &signature, &message);
        true
    }

    pub fn delete_wallet(&mut self, name: String, signature: String, expires_at: u64, nonce: u64) {
        self.assert_not_paused();
        self.verify_owner(&format!("delete_wallet:{}", name), &signature, expires_at, nonce);
        let wallet = self.wallet_get(&name).expect("ERR_WALLET_NOT_FOUND");

        for i in 0..wallet.intent_index {
            if let Some(intent) = self.intents.get(&intent_key(&name, i)) {
                assert!(
                    intent.active_proposal_count == 0,
                    "ERR_ACTIVE_PROPOSALS: intent #{} has {}",
                    i,
                    intent.active_proposal_count
                );
            }
        }

        // Collect delegation keys before removing intents
        let mut del_keys: Vec<String> = Vec::new();
        for i in 0..wallet.intent_index {
            if let Some(intent) = self.intents.get(&intent_key(&name, i)) {
                for j in 0..intent.approvers.len() {
                    del_keys.push(delegation_key(&name, i, j));
                }
            }
        }

        for i in 0..wallet.intent_index {
            self.intents.remove(&intent_key(&name, i));
        }
        for i in 0..wallet.proposal_index {
            self.proposals.remove(&proposal_key(&name, i));
        }
        for dkey in del_keys {
            self.delegations.remove(&dkey);
        }

        // Refund only the actual storage cost (20 yoctoNEAR per byte) + deposit remainder
        let storage_cost = NearToken::from_yoctonear(wallet.storage_used as u128)
    .saturating_mul(env::storage_byte_cost().as_yoctonear())
    .as_yoctonear();
        let refund = wallet.storage_deposit.saturating_sub(storage_cost);
        if refund > 0 {
            Promise::new(wallet.owner.clone()).transfer(NearToken::from_yoctonear(refund));
        }

        self.wallet_remove(&name);

        // Remove from wallet_names Vector (swap-remove)
        let len = self.wallet_names.len();
        let mut found_idx: Option<u64> = None;
        for i in 0..len {
            if let Some(n) = self.wallet_names.get(i) {
                if n == name {
                    found_idx = Some(i);
                    break;
                }
            }
        }
        if let Some(idx) = found_idx {
            self.wallet_names.swap_remove(idx);
        }

        self.emit_event("wallet_deleted", serde_json::json!({
            "wallet": name,
            "storage_used": wallet.storage_used,
            "refund": refund.to_string(),
        }));

        log!("Wallet '{}' deleted (refunded {} yocto)", name, refund);
    }

    pub fn transfer_ownership(&mut self, wallet_name: String, new_owner: AccountId, signature: String, expires_at: u64, nonce: u64) {
        self.assert_not_paused();
        self.verify_owner(&format!("transfer_ownership:{}", wallet_name), &signature, expires_at, nonce);
        let mut wallet = self.wallet_get_readonly(&wallet_name).expect("ERR_WALLET_NOT_FOUND");
        assert_ne!(new_owner, wallet.owner, "ERR_ALREADY_OWNER");

        let old_owner = wallet.owner.clone();
        wallet.owner = new_owner.clone();
        self.wallet_insert(&wallet_name, &wallet);

        for i in 0..3u32 {
            let ikey = intent_key(&wallet_name, i);
            if let Some(mut intent) = self.intents.get(&ikey) {
                if let Some(pos) = intent.proposers.iter().position(|a| a == &old_owner) {
                    intent.proposers[pos] = new_owner.clone();
                }
                if let Some(pos) = intent.approvers.iter().position(|a| a == &old_owner) {
                    intent.approvers[pos] = new_owner.clone();
                }
                self.intents.insert(&ikey, &intent);
            }
        }

        self.emit_event("ownership_transferred", serde_json::json!({
            "wallet": wallet_name,
            "old_owner": old_owner.to_string(),
            "new_owner": new_owner.to_string(),
        }));

        log!("Ownership of '{}' transferred to {}", wallet_name, new_owner);
    }

    /// Add an owner npub (requires an existing owner signature).
    pub fn add_owner_npub(&mut self, new_npub: String, signature: String, expires_at: u64, nonce: u64) {
        assert!(new_npub.len() == 64, "ERR_INVALID_NPUB_LEN: expected 64 hex chars");
        assert!(new_npub.chars().all(|c| c.is_ascii_hexdigit()), "ERR_INVALID_NPUB_HEX");
        self.verify_owner("add_owner_npub", &signature, expires_at, nonce);
        assert!(!self.owner_npubs.contains(&new_npub), "ERR_OWNER_ALREADY_PRESENT");
        assert!(self.owner_npubs.len() < 8, "ERR_TOO_MANY_OWNERS: max 8");
        self.owner_npubs.push(new_npub.clone());
        self.emit_event("owner_added", serde_json::json!({ "new_npub": new_npub }));
        log!("Owner npub added: {}", &new_npub[..16]);
    }

    /// Remove an owner npub. The last owner cannot be removed.
    pub fn remove_owner_npub(&mut self, npub: String, signature: String, expires_at: u64, nonce: u64) {
        self.verify_owner("remove_owner_npub", &signature, expires_at, nonce);
        assert!(self.owner_npubs.len() > 1, "ERR_LAST_OWNER: cannot remove the final owner");
        let before = self.owner_npubs.len();
        self.owner_npubs.retain(|p| p != &npub);
        assert!(self.owner_npubs.len() < before, "ERR_OWNER_NOT_FOUND");
        self.emit_event("owner_removed", serde_json::json!({ "npub": npub }));
        log!("Owner npub removed: {}", &npub[..16]);
    }

    /// Set (or clear with None) the guardian npub. Guardian may ONLY pause.
    pub fn set_guardian(&mut self, npub: Option<String>, signature: String, expires_at: u64, nonce: u64) {
        if let Some(ref n) = npub {
            assert!(n.len() == 64, "ERR_INVALID_NPUB_LEN: expected 64 hex chars");
            assert!(n.chars().all(|c| c.is_ascii_hexdigit()), "ERR_INVALID_NPUB_HEX");
        }
        self.verify_owner("set_guardian", &signature, expires_at, nonce);
        self.guardian_npub = npub.clone();
        self.emit_event("guardian_set", serde_json::json!({ "guardian_npub": npub }));
    }

    /// Get the owner nonce window base (for clients to pick a nonce in [base, base+64))
    pub fn get_owner_nonce(&self) -> u64 {
        self.owner_nonce
    }

    /// Get the consumed-nonce bitmap within the current window.
    pub fn get_owner_nonce_bitmap(&self) -> u64 {
        self.owner_nonce_bitmap
    }


    /// Get all owner npubs.
    pub fn get_owner_npubs(&self) -> Vec<String> {
        self.owner_npubs.clone()
    }

    /// Get the guardian npub, if set.
    pub fn get_guardian_npub(&self) -> Option<String> {
        self.guardian_npub.clone()
    }

    // ── Intent Management ──────────────────────────────────────────────

    /// Add a token to the wallet's FT allowlist. Owner only.
    /// Empty allowlist = accept all tokens.
    /// Once you add the first token, only listed tokens are accepted.
    pub fn add_allowed_token(&mut self, wallet_name: String, token: AccountId, signature: String, expires_at: u64, nonce: u64) {
        self.assert_not_paused();
        self.verify_owner(&format!("add_allowed_token:{}", wallet_name), &signature, expires_at, nonce);
        let mut wallet = self.wallet_get_readonly(&wallet_name).expect("ERR_WALLET_NOT_FOUND");
        assert!(
            !wallet.allowed_tokens.contains(&token),
            "ERR_TOKEN_ALREADY_ALLOWED"
        );
        wallet.allowed_tokens.push(token.clone());
        self.wallet_insert(&wallet_name, &wallet);

        self.emit_event("token_allowed", serde_json::json!({
            "wallet": wallet_name,
            "token": token.to_string(),
        }));

        log!("Token '{}' allowed for wallet '{}'", token, wallet_name);
    }

    /// Remove a token from the wallet's FT allowlist. Owner only.
    pub fn remove_allowed_token(&mut self, wallet_name: String, token: AccountId, signature: String, expires_at: u64, nonce: u64) {
        self.assert_not_paused();
        self.verify_owner(&format!("remove_allowed_token:{}", wallet_name), &signature, expires_at, nonce);
        let mut wallet = self.wallet_get_readonly(&wallet_name).expect("ERR_WALLET_NOT_FOUND");
        let original_len = wallet.allowed_tokens.len();
        wallet.allowed_tokens.retain(|t| t != &token);
        assert!(
            wallet.allowed_tokens.len() < original_len,
            "ERR_TOKEN_NOT_IN_LIST"
        );
        self.wallet_insert(&wallet_name, &wallet);

        self.emit_event("token_removed_from_allowlist", serde_json::json!({
            "wallet": wallet_name,
            "token": token.to_string(),
        }));

        log!("Token '{}' removed from allowlist for '{}'", token, wallet_name);
    }

    // ── Proposal Lifecycle ─────────────────────────────────────────────

    pub fn propose(
        &mut self,
        wallet_name: String,
        intent_index: u32,
        param_values: String,
        expires_at: u64,
        nonce: u64,
        signature: String,
    ) {
        self.assert_not_paused();
        // Signature binds to the proposal id that will be assigned
        let proposal_index = self
            .wallet_get_readonly(&wallet_name)
            .expect("ERR_WALLET_NOT_FOUND")
            .proposal_index;
        // Verify nostr owner signature
        self.verify_owner(&format!("propose:{}:{}", wallet_name, proposal_index), &signature, expires_at, nonce);
        self.propose_inner(wallet_name, intent_index, param_values, expires_at);
    }

    /// Propose logic shared by the relayer path (`propose`) and the session-key
    /// path (`submit_action`). Auth is verified by the caller.
    fn propose_inner(
        &mut self,
        wallet_name: String,
        intent_index: u32,
        param_values: String,
        expires_at: u64,
    ) {
        let mut wallet = self.wallet_get_readonly(&wallet_name).expect("ERR_WALLET_NOT_FOUND");
        let ikey = intent_key(&wallet_name, intent_index);
        let intent = self.intents.get(&ikey).expect("ERR_INTENT_NOT_FOUND");

        assert!(intent.active, "ERR_INTENT_INACTIVE");
        assert!(expires_at > env::block_timestamp(), "ERR_EXPIRED");
        assert!(expires_at <= env::block_timestamp() + MAX_EXPIRY_NS, "ERR_EXPIRY_TOO_FAR");
        assert!(intent.active_proposal_count < MAX_ACTIVE_PROPOSALS, "ERR_MAX_PROPOSALS");

        let params: serde_json::Value = serde_json::from_str(&param_values).expect("ERR_INVALID_JSON");
        self.validate_params(&intent, &params);

        let proposal_index = wallet.proposal_index;
        let msg = message::build_message(&wallet_name, proposal_index, expires_at, "propose", &intent, &params);

        let proposal = Proposal {
            id: proposal_index,
            wallet_name: wallet_name.clone(),
            intent_index,
            proposer: env::predecessor_account_id(),
            status: ProposalStatus::Active,
            proposed_at: env::block_timestamp(),
            approved_at: 0,
            expires_at,
            approval_bitmap: 0,
            cancellation_bitmap: 0,
            nostr_approval_bitmap: 0,
            nostr_cancellation_bitmap: 0,
            param_values,
            message: msg.clone(),
            intent_params_hash: hash_params(&intent.params),
        };

        self.proposals.insert(&proposal_key(&wallet_name, proposal_index), &proposal);

        let mut intent_mut = intent.clone();
        intent_mut.active_proposal_count += 1;
        self.intents.insert(&ikey, &intent_mut);

        wallet.proposal_index = proposal_index + 1;
        self.wallet_insert(&wallet_name, &wallet);

        self.emit_event("proposal_created", serde_json::json!({
            "wallet": wallet_name, "proposal_id": proposal_index,
            "intent_index": intent_index, "message": msg,
        }));

        log!("Proposal #{} created for intent #{}", proposal_index, intent_index);
    }

    pub fn amend_proposal(
        &mut self,
        wallet_name: String,
        proposal_id: u64,
        param_values: String,
        expires_at: u64,
        nonce: u64,
        signature: String,
    ) {
        self.assert_not_paused();
        self.verify_owner(&format!("amend:{}:{}", wallet_name, proposal_id), &signature, expires_at, nonce);
        self.amend_inner(wallet_name, proposal_id, param_values, expires_at);
    }

    /// Amend logic shared by the relayer path and the session-key path.
    /// Auth is verified by the caller.
    fn amend_inner(
        &mut self,
        wallet_name: String,
        proposal_id: u64,
        param_values: String,
        expires_at: u64,
    ) {
        let pkey = proposal_key(&wallet_name, proposal_id);
        let mut proposal = self.proposals.get(&pkey).expect("ERR_PROPOSAL_NOT_FOUND");

        assert!(proposal.status == ProposalStatus::Active, "ERR_NOT_ACTIVE");
        assert!(expires_at > env::block_timestamp(), "ERR_EXPIRED");
        assert!(expires_at <= env::block_timestamp() + MAX_EXPIRY_NS, "ERR_EXPIRY_TOO_FAR");

        let ikey = intent_key(&wallet_name, proposal.intent_index);
        let intent = self.intents.get(&ikey).expect("ERR_INTENT_NOT_FOUND");
        assert!(intent.active, "ERR_INTENT_INACTIVE");

        let params: serde_json::Value = serde_json::from_str(&param_values).expect("ERR_INVALID_JSON");
        self.validate_params(&intent, &params);

        let msg = message::build_message(&wallet_name, proposal_id, expires_at, "amend", &intent, &params);

        proposal.reset_votes();
        proposal.param_values = param_values;
        proposal.expires_at = expires_at;
        proposal.message = msg;
        proposal.intent_params_hash = hash_params(&intent.params);

        self.proposals.insert(&pkey, &proposal);

        self.emit_event("proposal_amended", serde_json::json!({
            "wallet": wallet_name, "proposal_id": proposal_id,
        }));

        log!("Proposal #{} amended", proposal_id);
    }

    /// Approve a proposal using a nostr schnorr signature.
    /// `approver_index` indexes into `intent.nostr_approvers`.
    pub fn approve(
        &mut self,
        wallet_name: String,
        proposal_id: u64,
        approver_index: u16,
        pubkey_hex: String,
        signature: String,
        expires_at: u64,
    ) {
        self.assert_not_paused();
        self.verify_nostr_approver(
            wallet_name, proposal_id, approver_index, pubkey_hex, signature, expires_at, "approve",
        );
    }

    /// Cancel-vote a proposal using a nostr schnorr signature.
    pub fn cancel_vote(
        &mut self,
        wallet_name: String,
        proposal_id: u64,
        approver_index: u16,
        pubkey_hex: String,
        signature: String,
        expires_at: u64,
    ) {
        self.assert_not_paused();
        self.verify_nostr_approver(
            wallet_name, proposal_id, approver_index, pubkey_hex, signature, expires_at, "cancel",
        );
    }
    /// Approve a proposal using a signed nostr event (NIP-46 compatible).
    ///
    /// The event must be:
    ///   kind: 37500
    ///   content: the exact proposal message (from get_proposal_message)
    ///   tags: [["contract", "<id>"], ["wallet", "<name>"], ["proposal", "<id>"]]
    ///
    /// The contract verifies:
    ///   1. Event ID = SHA256(serialization)  — content wasn't tampered
    ///   2. Schnorr(pubkey, sig, event_id)      — signed by the key owner
    ///   3. content == proposal.message         — matches this specific proposal
    ///   4. pubkey == nostr_approvers[idx]      — authorized approver
    pub fn approve_with_event(
        &mut self,
        wallet_name: String,
        proposal_id: u64,
        approver_index: u16,
        pubkey_hex: String,
        event_id_hex: String,
        created_at: u64,
        kind: u32,
        tags_json: String,
        content: String,
        sig_hex: String,
    ) {
        self.assert_not_paused();

        // Verify the event signature
        nostr_event::verify_event(
            &pubkey_hex, &event_id_hex, created_at, kind, &tags_json, &content, &sig_hex,
        );

        // Load proposal and intent
        let pkey = proposal_key(&wallet_name, proposal_id);
        let mut proposal = self.proposals.get(&pkey).expect("ERR_PROPOSAL_NOT_FOUND");
        assert!(proposal.status == ProposalStatus::Active, "ERR_NOT_ACTIVE");
        assert!(proposal.expires_at > env::block_timestamp(), "ERR_PROPOSAL_EXPIRED");

        let ikey = intent_key(&wallet_name, proposal.intent_index);
        let intent = self.intents.get(&ikey).expect("ERR_INTENT_NOT_FOUND");
        assert!((approver_index as usize) < intent.nostr_approvers.len(), "ERR_INVALID_NOSTR_APPROVER_INDEX");
        assert_eq!(pubkey_hex, intent.nostr_approvers[approver_index as usize], "ERR_NOSTR_PK_MISMATCH");

        // Verify the event content matches the proposal message
        assert_eq!(content, proposal.message, "ERR_EVENT_CONTENT_MISMATCH");

        // Process the approval
        assert!(!proposal.has_nostr_approved(approver_index as usize), "ERR_NOSTR_ALREADY_APPROVED");
        proposal.set_nostr_approval(approver_index as usize);

        if proposal.approval_count() >= intent.approval_threshold as u32 {
            proposal.status = ProposalStatus::Approved;
            proposal.approved_at = env::block_timestamp();
            self.emit_event("proposal_approved", serde_json::json!({
                "wallet": wallet_name, "proposal_id": proposal_id,
                "approval_count": proposal.approval_count(),
                "nostr": true, "via_event": true,
            }));
            log!("Proposal #{} approved (nostr event)", proposal_id);
        }

        self.proposals.insert(&pkey, &proposal);
    }

    /// Cancel-vote a proposal using a signed nostr event (NIP-46 compatible).
    pub fn cancel_vote_with_event(
        &mut self,
        wallet_name: String,
        proposal_id: u64,
        approver_index: u16,
        pubkey_hex: String,
        event_id_hex: String,
        created_at: u64,
        kind: u32,
        tags_json: String,
        content: String,
        sig_hex: String,
    ) {
        self.assert_not_paused();

        nostr_event::verify_event(
            &pubkey_hex, &event_id_hex, created_at, kind, &tags_json, &content, &sig_hex,
        );

        let pkey = proposal_key(&wallet_name, proposal_id);
        let mut proposal = self.proposals.get(&pkey).expect("ERR_PROPOSAL_NOT_FOUND");
        assert!(proposal.status == ProposalStatus::Active, "ERR_NOT_ACTIVE");
        assert!(proposal.expires_at > env::block_timestamp(), "ERR_PROPOSAL_EXPIRED");

        let ikey = intent_key(&wallet_name, proposal.intent_index);
        let intent = self.intents.get(&ikey).expect("ERR_INTENT_NOT_FOUND");
        assert!((approver_index as usize) < intent.nostr_approvers.len(), "ERR_INVALID_NOSTR_APPROVER_INDEX");
        assert_eq!(pubkey_hex, intent.nostr_approvers[approver_index as usize], "ERR_NOSTR_PK_MISMATCH");
        assert_eq!(content, proposal.message, "ERR_EVENT_CONTENT_MISMATCH");

        proposal.set_nostr_cancellation(approver_index as usize);
        if proposal.cancellation_count() >= intent.cancellation_threshold as u32 {
            proposal.status = ProposalStatus::Cancelled;
            let mut intent_mut = intent.clone();
            intent_mut.active_proposal_count = intent_mut.active_proposal_count.saturating_sub(1);
            self.intents.insert(&ikey, &intent_mut);
            self.emit_event("proposal_cancelled", serde_json::json!({
                "wallet": wallet_name, "proposal_id": proposal_id,
                "cancellation_count": proposal.cancellation_count(),
                "nostr": true, "via_event": true,
            }));
        }

        self.proposals.insert(&pkey, &proposal);
    }



    // ── Quick Execute (Solo User) ────────────────────────────────────

    /// One-call execution for solo users. Proposes, auto-approves, and
    /// executes in a single transaction. Only works when:
    /// - The caller is the owner (verified via nostr signature)
    /// - The intent has approval_threshold == 1
    /// - The owner's npub is in intent.nostr_approvers
    ///
    /// This lets a Nostr user do everything with a single signed message.
    #[payable]
    pub fn quick_execute(
        &mut self,
        wallet_name: String,
        intent_index: u32,
        param_values: String,
        expires_at: u64,
        nonce: u64,
        signature: String,
    ) {
        self.assert_not_paused();
        // Verify owner signature for this action
        let action = format!("quick:{}:{}:{}", wallet_name, intent_index, &param_values.chars().take(64).collect::<String>());
        self.verify_owner(&action, &signature, expires_at, nonce);
        self.quick_execute_inner(wallet_name, intent_index, param_values, expires_at, false);
    }

    /// Quick-execute logic shared by the relayer path and the session-key path.
    /// When `via_session` is true the predecessor-based proposer check is
    /// skipped (under a gas key the predecessor is the contract itself; the
    /// owner's nostr signature over the full action string is the authority).
    fn quick_execute_inner(
        &mut self,
        wallet_name: String,
        intent_index: u32,
        param_values: String,
        expires_at: u64,
        via_session: bool,
    ) {
        let ikey = intent_key(&wallet_name, intent_index);
        let intent = self.intents.get(&ikey).expect("ERR_INTENT_NOT_FOUND");
        assert!(intent.active, "ERR_INTENT_INACTIVE");
        assert!(intent.approval_threshold == 1, "ERR_NOT_SOLO: quick_execute only works with approval_threshold=1");

        // Check owner is a proposer
        if !via_session {
            let predecessor = env::predecessor_account_id();
            let owner_is_proposer = intent.proposers.is_empty() || intent.proposers.contains(&predecessor);
            assert!(owner_is_proposer, "ERR_NOT_PROPOSER: caller not in intent proposers");
        }

        // Check owner is in nostr_approvers
        let owner_is_approver = intent.nostr_approvers.iter().any(|p| self.owner_npubs.contains(p));
        assert!(owner_is_approver, "ERR_OWNER_NOT_APPROVER: owner must be in nostr_approvers");

        // Validate params
        let params: serde_json::Value = serde_json::from_str(&param_values).expect("ERR_INVALID_JSON");
        self.validate_params(&intent, &params);

        // Create proposal
        let wallet = self.wallet_get_readonly(&wallet_name).expect("ERR_WALLET_NOT_FOUND");
        let proposal_id = wallet.proposal_index;
        let msg = message::build_message(&wallet_name, proposal_id, expires_at, "quick", &intent, &params);

        let proposal = Proposal {
            id: proposal_id,
            wallet_name: wallet_name.clone(),
            intent_index,
            proposer: env::predecessor_account_id(),
            status: ProposalStatus::Approved, // Auto-approved
            proposed_at: env::block_timestamp(),
            approved_at: env::block_timestamp(),
            expires_at,
            approval_bitmap: 0,
            cancellation_bitmap: 0,
            nostr_approval_bitmap: 1u64 << 0, // Owner approved
            nostr_cancellation_bitmap: 0,
            param_values: param_values.clone(),
            message: msg,
            intent_params_hash: hash_params(&intent.params),
        };

        let pkey = proposal_key(&wallet_name, proposal_id);
        self.proposals.insert(&pkey, &proposal);

        // Update wallet proposal index
        let mut wallet = self.wallet_get_readonly(&wallet_name).expect("ERR_WALLET_NOT_FOUND");
        wallet.proposal_index += 1;
        self.wallet_insert(&wallet_name, &wallet);

        // Update intent active_proposal_count
        let mut intent = intent;
        intent.active_proposal_count += 1;
        self.intents.insert(&ikey, &intent);

        self.emit_event("quick_proposed_and_approved", serde_json::json!({
            "wallet": wallet_name, "proposal_id": proposal_id, "intent_index": intent_index,
        }));
        log!("Proposal #{} quick-approved for intent #{}", proposal_id, intent_index);

        // Now execute it (inline the execution logic)
        self.execute_proposal(&wallet_name, proposal_id);
    }

    // ── Execution ────────────────────────────────────────────────────

    /// Execute an approved proposal. Requires owner nostr signature.
    /// Payable: allows attached deposit for "deposit NEAR" intent executions.
    #[payable]
    pub fn execute(&mut self, wallet_name: String, proposal_id: u64, signature: String, expires_at: u64, nonce: u64) {
        self.assert_not_paused();
        self.verify_owner(&format!("execute:{}:{}", wallet_name, proposal_id), &signature, expires_at, nonce);
        self.execute_proposal(&wallet_name, proposal_id);
    }

    /// Internal execution logic shared by execute() and quick_execute().
    fn execute_proposal(&mut self, wallet_name: &String, proposal_id: u64) {
        let pkey = proposal_key(wallet_name, proposal_id);
        let mut proposal = self.proposals.get(&pkey).expect("ERR_PROPOSAL_NOT_FOUND");
        assert!(proposal.status == ProposalStatus::Approved, "ERR_NOT_APPROVED");
        assert!(proposal.expires_at > env::block_timestamp(), "ERR_PROPOSAL_EXPIRED");

        let ikey = intent_key(wallet_name, proposal.intent_index);
        let intent = self.intents.get(&ikey).expect("ERR_INTENT_NOT_FOUND");
        
        // Check timelock
        if intent.timelock_seconds > 0 {
            let elapsed = env::block_timestamp() as u128 - proposal.proposed_at as u128;
            let timelock_ns = (intent.timelock_seconds as u128) * 1_000_000_000u128;
            assert!(elapsed >= timelock_ns, "ERR_TIMELOCK_NOT_EXPIRED");
        }

        // Verify params haven't changed since proposal
        let current_hash = hash_params(&intent.params);
        assert_eq!(
            proposal.intent_params_hash, current_hash,
            "ERR_PARAMS_CHANGED: intent schema was modified after proposal"
        );

        let params: serde_json::Value = serde_json::from_str(&proposal.param_values).unwrap_or_default();
        let definition = params.get("definition").and_then(|v| v.as_str());

        match intent.intent_type {
            IntentType::AddIntent => {
                let new_intent: Intent = if let Some(def) = definition {
                    near_sdk::serde_json::from_str(def)
                        .expect("ERR_INVALID_INTENT_DEFINITION")
                } else if let Some(def_obj) = params.get("definition") {
                    near_sdk::serde_json::from_value(def_obj.clone())
                        .expect("ERR_INVALID_INTENT_DEFINITION")
                } else {
                    // Build intent from top-level params with defaults
                    let proposers: Vec<AccountId> = params.get("proposers")
                        .and_then(|v| v.as_array())
                        .map(|arr| arr.iter().filter_map(|v| v.as_str().and_then(|s| s.parse().ok())).collect())
                        .unwrap_or_default();
                    let approvers: Vec<AccountId> = params.get("approvers")
                        .and_then(|v| v.as_array())
                        .map(|arr| arr.iter().filter_map(|v| v.as_str().and_then(|s| s.parse().ok())).collect())
                        .unwrap_or_default();
                    let pdefs: Vec<ParamDef> = params.get("params")
                        .and_then(|v| v.as_array())
                        .map(|arr| arr.iter().filter_map(|v| near_sdk::serde_json::from_value(v.clone()).ok()).collect())
                        .unwrap_or_default();
                    Intent {
                        wallet_name: wallet_name.clone(),
                        index: 0, // will be overwritten
                        intent_type: IntentType::Custom,
                        name: params.get("name").and_then(|v| v.as_str()).unwrap_or("Custom").to_string(),
                        template: params.get("template").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                        proposers,
                        approvers,
                        nostr_approvers: params.get("nostr_approvers")
                            .and_then(|v| v.as_array())
                            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
                            .unwrap_or_default(),
                        approval_threshold: params.get("approval_threshold").and_then(|v| v.as_u64()).unwrap_or(1) as u16,
                        cancellation_threshold: params.get("cancellation_threshold").and_then(|v| v.as_u64()).unwrap_or(1) as u16,
                        timelock_seconds: params.get("timelock_seconds").and_then(|v| v.as_u64()).unwrap_or(0),
                        params: pdefs,
                        execution_gas_tgas: params.get("execution_gas_tgas").and_then(|v| v.as_u64()).unwrap_or(DEFAULT_EXECUTION_GAS_TGAS),
                        active: true,
                        active_proposal_count: 0,
                    }
                };
                let mut wallet = self.wallet_get_readonly(&wallet_name).expect("ERR_WALLET_NOT_FOUND");
                let new_index = wallet.intent_index;
                self.intents.insert(&intent_key(&wallet_name, new_index), &new_intent);
                wallet.intent_index += 1;
                self.wallet_insert(&wallet_name, &wallet);
                log!("Intent #{} added to wallet {}", new_index, wallet_name);
            }
            IntentType::RemoveIntent => {
                let idx = params["index"].as_u64().expect("ERR_MISSING_INDEX") as u32;
                let mut ri = self.intents.get(&intent_key(&wallet_name, idx)).expect("ERR_INTENT_NOT_FOUND");
                ri.active = false;
                self.intents.insert(&intent_key(&wallet_name, idx), &ri);
                log!("Intent #{} deactivated", idx);
            }
            IntentType::UpdateIntent => {
                let idx = params["index"].as_u64().expect("ERR_MISSING_INDEX") as u32;
                let def = params.get("definition").and_then(|v| v.as_str());
                let updated: Intent = if let Some(d) = def {
                    near_sdk::serde_json::from_str(d).expect("ERR_INVALID_DEFINITION")
                } else {
                    // Merge: start from existing intent, overlay params
                    let mut existing = self.intents.get(&intent_key(&wallet_name, idx)).expect("ERR_INTENT_NOT_FOUND");
                    if let Some(v) = params.get("name").and_then(|v| v.as_str()) { existing.name = v.to_string(); }
                    if let Some(v) = params.get("template").and_then(|v| v.as_str()) { existing.template = v.to_string(); }
                    if let Some(v) = params.get("approval_threshold").and_then(|v| v.as_u64()) { existing.approval_threshold = v as u16; }
                    if let Some(v) = params.get("cancellation_threshold").and_then(|v| v.as_u64()) { existing.cancellation_threshold = v as u16; }
                    if let Some(v) = params.get("timelock_seconds").and_then(|v| v.as_u64()) { existing.timelock_seconds = v; }
                    if let Some(v) = params.get("execution_gas_tgas").and_then(|v| v.as_u64()) { existing.execution_gas_tgas = v; }
                    if let Some(v) = params.get("active").and_then(|v| v.as_bool()) { existing.active = v; }
                    if let Some(arr) = params.get("proposers").and_then(|v| v.as_array()) {
                        existing.proposers = arr.iter().filter_map(|v| v.as_str().and_then(|s| s.parse().ok())).collect();
                    }
                    if let Some(arr) = params.get("approvers").and_then(|v| v.as_array()) {
                        existing.approvers = arr.iter().filter_map(|v| v.as_str().and_then(|s| s.parse().ok())).collect();
                    }
                    if let Some(arr) = params.get("nostr_approvers").and_then(|v| v.as_array()) {
                        existing.nostr_approvers = arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect();
                    }
                    if let Some(arr) = params.get("params").and_then(|v| v.as_array()) {
                        existing.params = arr.iter().filter_map(|v| near_sdk::serde_json::from_value(v.clone()).ok()).collect();
                    }
                    existing
                };
                self.intents.insert(&intent_key(&wallet_name, idx), &updated);
                log!("Intent #{} updated", idx);
            }
            IntentType::Custom => {
                // Keep string-matching for backwards compat with existing intents
                if intent.template.contains("deposit") || intent.name.to_lowercase().contains("deposit") {
                    self.execute_deposit(&wallet_name);
                } else if intent.template.contains("transfer") || intent.name.contains("transfer") {
                    self.execute_transfer(&wallet_name, &params);
                } else {
                    let truncated: String = proposal.param_values.chars().take(200).collect();
                    log!("Custom '{}' executed: {}", intent.name, truncated);
                }
            }
            IntentType::Deposit => {
                self.execute_deposit(&wallet_name);
            }
            IntentType::Transfer => {
                self.execute_transfer(&wallet_name, &params);
            }
            IntentType::Call => {
                self.execute_call(&wallet_name, &params);
            }
        }

        proposal.status = ProposalStatus::Executed;
        self.proposals.insert(&pkey, &proposal);
        let mut intent_mut = intent.clone();
        intent_mut.active_proposal_count = intent_mut.active_proposal_count.saturating_sub(1);
        self.intents.insert(&ikey, &intent_mut);

        // ── Relayer payout: reward the tx submitter from wallet balance ──
        // (gas + service fee). Only if the relayer is allowed and a fee is set.
        let relayer = env::predecessor_account_id();
        if relayer != env::current_account_id() {
            if let Some(mut wallet) = self.wallet_get_readonly(wallet_name) {
                if wallet.is_relayer_allowed(&relayer) && wallet.relayer_fee > 0 {
                    let bal = self.get_wallet_near_balance(wallet_name.clone()).0;
                    let payout = wallet.relayer_fee.min(bal);
                    if payout > 0 {
                        self.debit_near(wallet_name, payout);
                        wallet.storage_used = wallet.storage_used; // no-op keep borrow
                        Promise::new(relayer.clone()).transfer(near_sdk::NearToken::from_yoctonear(payout));
                        log!("Relayer fee paid: {} yoctoNEAR to {}", payout, relayer);
                    }
                }
                let _ = &mut wallet;
            }
        }

        self.emit_event("proposal_executed", serde_json::json!({
            "wallet": wallet_name, "proposal_id": proposal_id,
        }));
    }

    /// Remove an executed/cancelled proposal to reclaim storage. Owner only.
    pub fn cleanup(&mut self, wallet_name: String, proposal_id: u64, signature: String, expires_at: u64, nonce: u64) {
        self.assert_not_paused();
        self.verify_owner(&format!("cleanup:{}:{}", wallet_name, proposal_id), &signature, expires_at, nonce);

        let pkey = proposal_key(&wallet_name, proposal_id);
        let proposal = self.proposals.get(&pkey).expect("ERR_PROPOSAL_NOT_FOUND");
        assert!(
            proposal.status == ProposalStatus::Executed || proposal.status == ProposalStatus::Cancelled,
            "ERR_NOT_EXECUTABLE: only executed or cancelled proposals can be cleaned up"
        );

        self.proposals.remove(&pkey);
        self.emit_event("proposal_cleaned", serde_json::json!({
            "wallet": wallet_name, "proposal_id": proposal_id,
        }));
        log!("Proposal #{} cleaned up from wallet '{}'", proposal_id, wallet_name);
    }

    // ── Views ──────────────────────────────────────────────────────────

    pub fn get_wallet(&self, name: String) -> Option<Wallet> {
        self.wallet_get_readonly(&name)
    }

    pub fn get_intent(&self, wallet_name: String, index: u32) -> Option<Intent> {
        self.intents.get(&intent_key(&wallet_name, index))
    }

    pub fn list_intents(&self, wallet_name: String) -> Vec<Intent> {
        let Some(wallet) = self.wallet_get_readonly(&wallet_name) else { return Vec::new(); };
        (0..wallet.intent_index)
            .filter_map(|i| self.intents.get(&intent_key(&wallet_name, i)))
            .collect()
    }

    pub fn get_proposal(&self, wallet_name: String, id: u64) -> Option<Proposal> {
        self.proposals.get(&proposal_key(&wallet_name, id))
    }

    /// List proposals — capped at 100 to prevent gas exhaustion.
    /// Use get_proposals_paginated for large wallets.
    pub fn list_proposals(&self, wallet_name: String) -> Vec<Proposal> {
        let Some(wallet) = self.wallet_get_readonly(&wallet_name) else { return Vec::new(); };
        let limit = wallet.proposal_index.min(100);
        let start = wallet.proposal_index.saturating_sub(limit);
        (start..wallet.proposal_index)
            .filter_map(|i| self.proposals.get(&proposal_key(&wallet_name, i)))
            .collect()
    }

    pub fn get_proposal_message(&self, wallet_name: String, id: u64) -> Option<String> {
        self.proposals.get(&proposal_key(&wallet_name, id)).map(|p| p.message)
    }

    pub fn get_allowed_tokens(&self, wallet_name: String) -> Vec<AccountId> {
        self.wallet_get_readonly(&wallet_name)
            .map(|w| w.allowed_tokens)
            .unwrap_or_default()
    }

    pub fn get_delegation(&self, wallet_name: String, intent_index: u32, approver_index: u16) -> Option<AccountId> {
        self.delegations.get(&delegation_key(&wallet_name, intent_index, approver_index as usize))
    }

    pub fn get_event_nonce(&self) -> u64 {
        self.event_nonce
    }

    /// List all wallet names on this contract.
    pub fn list_wallets(&self, from_index: u64, limit: u64) -> Vec<String> {
        let start = from_index.min(self.wallet_names.len());
        let end = (start + limit).min(self.wallet_names.len());
        (start..end)
            .filter_map(|i| self.wallet_names.get(i))
            .collect()
    }

    /// Get total number of wallets.
    pub fn get_wallet_count(&self) -> u64 {
        self.wallet_names.len()
    }

    /// Get comprehensive wallet state: wallet, intents, balances, recent proposals
    pub fn get_wallet_state(&self, wallet_name: String) -> serde_json::Value {
        let wallet = match self.wallet_get_readonly(&wallet_name) {
            Some(w) => w,
            None => return serde_json::json!({"error": "ERR_WALLET_NOT_FOUND"}),
        };

        let intents: Vec<Intent> = (0..wallet.intent_index)
            .filter_map(|i| self.intents.get(&intent_key(&wallet_name, i)))
            .collect();

        let near_balance = self.get_wallet_near_balance(wallet_name.clone()).0;

        // Get recent proposals (last 10)
        let proposals: Vec<Proposal> = if wallet.proposal_index > 10 {
            (wallet.proposal_index - 10..wallet.proposal_index)
                .filter_map(|i| self.proposals.get(&proposal_key(&wallet_name, i)))
                .collect()
        } else {
            (0..wallet.proposal_index)
                .filter_map(|i| self.proposals.get(&proposal_key(&wallet_name, i)))
                .collect()
        };

        // serde_json cannot serialize raw u128 — stringify the wallet fields.
        let wallet_json = serde_json::json!({
            "name": wallet.name,
            "owner": wallet.owner,
            "proposal_index": wallet.proposal_index,
            "intent_index": wallet.intent_index,
            "created_at": wallet.created_at,
            "storage_deposit": wallet.storage_deposit.to_string(),
            "storage_used": wallet.storage_used,
            "allowed_tokens": wallet.allowed_tokens,
            "ft_token_count": wallet.ft_token_count,
            "call_allowed_receivers": wallet.call_allowed_receivers,
            "call_max_deposit": wallet.call_max_deposit.to_string(),
            "daily_spend_limit": wallet.daily_spend_limit.to_string(),
            "daily_spend_reset_at": wallet.daily_spend_reset_at,
            "daily_spend_used": wallet.daily_spend_used.to_string(),
            "relayer_fee": wallet.relayer_fee.to_string(),
            "allowed_relayers": wallet.allowed_relayers,
        });
        serde_json::json!({
            "wallet": wallet_json,
            "intents": intents,
            "near_balance": near_balance.to_string(),
            "recent_proposals": proposals,
        })
    }

    /// Get paginated proposal history
    pub fn get_proposals_paginated(
        &self,
        wallet_name: String,
        from: u64,
        limit: u64,
    ) -> Vec<Proposal> {
        let wallet = match self.wallet_get_readonly(&wallet_name) {
            Some(w) => w,
            None => return Vec::new(),
        };
        let start = from.min(wallet.proposal_index);
        let end = (start + limit).min(wallet.proposal_index);
        (start..end)
            .filter_map(|i| self.proposals.get(&proposal_key(&wallet_name, i)))
            .collect()
    }

    /// Get daily spend stats for a wallet
    pub fn get_spend_stats(&self, wallet_name: String) -> serde_json::Value {
        let wallet = match self.wallet_get_readonly(&wallet_name) {
            Some(w) => w,
            None => return serde_json::json!({"error": "ERR_WALLET_NOT_FOUND"}),
        };
        serde_json::json!({
            "daily_limit": wallet.daily_spend_limit.to_string(),
            "daily_used": wallet.daily_spend_used.to_string(),
            "daily_remaining": wallet.daily_spend_limit.saturating_sub(wallet.daily_spend_used).to_string(),
            "reset_at": wallet.daily_spend_reset_at.to_string(),
            "call_max_deposit": wallet.call_max_deposit.to_string(),
        })
    }

    // ── Wallet Configuration (Owner Only) ────────────────────────────

    /// Set Call intent receiver allowlist. Empty = all allowed.
    pub fn set_call_allowed_receivers(
        &mut self,
        wallet_name: String,
        receivers: Vec<AccountId>,
        signature: String,
        expires_at: u64,
        nonce: u64,
    ) {
        self.assert_not_paused();
        self.verify_owner(&format!("set_call_receivers:{}", wallet_name), &signature, expires_at, nonce);
        let mut wallet = self.wallet_get_readonly(&wallet_name).expect("ERR_WALLET_NOT_FOUND");
        wallet.call_allowed_receivers = receivers;
        self.wallet_insert(&wallet_name, &wallet);
        self.emit_event("call_receivers_updated", serde_json::json!({"wallet": wallet_name}));
        log!("Call receiver allowlist updated for '{}'", wallet_name);
    }

    /// Set max deposit per Call intent (0 = no limit)
    pub fn set_call_max_deposit(
        &mut self,
        wallet_name: String,
        max_deposit: U128,
        signature: String,
        expires_at: u64,
        nonce: u64,
    ) {
        self.assert_not_paused();
        self.verify_owner(&format!("set_call_max_deposit:{}", wallet_name), &signature, expires_at, nonce);
        let mut wallet = self.wallet_get_readonly(&wallet_name).expect("ERR_WALLET_NOT_FOUND");
        wallet.call_max_deposit = max_deposit.0;
        self.wallet_insert(&wallet_name, &wallet);
        self.emit_event("call_max_deposit_updated", serde_json::json!({"wallet": wallet_name, "max": max_deposit.0.to_string()}));
        log!("Call max deposit set to {} for '{}'", max_deposit.0, wallet_name);
    }

    /// Set daily spend limit (0 = no limit)
    pub fn set_daily_spend_limit(
        &mut self,
        wallet_name: String,
        limit: U128,
        signature: String,
        expires_at: u64,
        nonce: u64,
    ) {
        self.assert_not_paused();
        self.verify_owner(&format!("set_daily_limit:{}", wallet_name), &signature, expires_at, nonce);
        let mut wallet = self.wallet_get_readonly(&wallet_name).expect("ERR_WALLET_NOT_FOUND");
        wallet.daily_spend_limit = limit.0;
        self.wallet_insert(&wallet_name, &wallet);
        self.emit_event("daily_limit_updated", serde_json::json!({"wallet": wallet_name, "limit": limit.0.to_string()}));
        log!("Daily spend limit set to {} for '{}'", limit.0, wallet_name);
    }

    /// Set relayer fee (charged from wallet storage deposit per execution)
    pub fn set_relayer_fee(
        &mut self,
        wallet_name: String,
        fee: U128,
        signature: String,
        expires_at: u64,
        nonce: u64,
    ) {
        self.assert_not_paused();
        self.verify_owner(&format!("set_relayer_fee:{}", wallet_name), &signature, expires_at, nonce);
        let mut wallet = self.wallet_get_readonly(&wallet_name).expect("ERR_WALLET_NOT_FOUND");
        wallet.relayer_fee = fee.0;
        self.wallet_insert(&wallet_name, &wallet);
        self.emit_event("relayer_fee_updated", serde_json::json!({"wallet": wallet_name, "fee": fee.0.to_string()}));
        log!("Relayer fee set to {} for '{}'", fee.0, wallet_name);
    }

    /// Set allowed relayers (empty = anyone can relay)
    pub fn set_allowed_relayers(
        &mut self,
        wallet_name: String,
        relayers: Vec<AccountId>,
        signature: String,
        expires_at: u64,
        nonce: u64,
    ) {
        self.assert_not_paused();
        self.verify_owner(&format!("set_relayers:{}", wallet_name), &signature, expires_at, nonce);
        let mut wallet = self.wallet_get_readonly(&wallet_name).expect("ERR_WALLET_NOT_FOUND");
        wallet.allowed_relayers = relayers;
        self.wallet_insert(&wallet_name, &wallet);
        self.emit_event("relayers_updated", serde_json::json!({"wallet": wallet_name}));
        log!("Relayer allowlist updated for '{}'", wallet_name);
    }

    // ── Intent Activation ─────────────────────────────────────────────

    /// Activate an intent (undo deactivation)
    pub fn activate_intent(
        &mut self,
        wallet_name: String,
        intent_index: u32,
        signature: String,
        expires_at: u64,
        nonce: u64,
    ) {
        self.assert_not_paused();
        self.verify_owner(&format!("activate_intent:{}:{}", wallet_name, intent_index), &signature, expires_at, nonce);
        let ikey = intent_key(&wallet_name, intent_index);
        let mut intent = self.intents.get(&ikey).expect("ERR_INTENT_NOT_FOUND");
        intent.active = true;
        self.intents.insert(&ikey, &intent);
        self.emit_event("intent_activated", serde_json::json!({"wallet": wallet_name, "intent_index": intent_index}));
        log!("Intent #{} activated in wallet '{}'", intent_index, wallet_name);
    }

    /// Deactivate an intent (pause without removing)
    pub fn deactivate_intent(
        &mut self,
        wallet_name: String,
        intent_index: u32,
        signature: String,
        expires_at: u64,
        nonce: u64,
    ) {
        self.assert_not_paused();
        self.verify_owner(&format!("deactivate_intent:{}:{}", wallet_name, intent_index), &signature, expires_at, nonce);
        let ikey = intent_key(&wallet_name, intent_index);
        let mut intent = self.intents.get(&ikey).expect("ERR_INTENT_NOT_FOUND");
        assert!(intent.active_proposal_count == 0, "ERR_HAS_ACTIVE_PROPOSALS");
        intent.active = false;
        self.intents.insert(&ikey, &intent);
        self.emit_event("intent_deactivated", serde_json::json!({"wallet": wallet_name, "intent_index": intent_index}));
        log!("Intent #{} deactivated in wallet '{}'", intent_index, wallet_name);
    }

    // ── Batch Execution ───────────────────────────────────────────────

    /// Execute multiple approved proposals in a single transaction.
    /// Owner signs once with the batch proposal IDs.
    #[payable]
    pub fn batch_execute(
        &mut self,
        wallet_name: String,
        proposal_ids: Vec<u64>,
        signature: String,
        expires_at: u64,
        nonce: u64,
    ) {
        self.assert_not_paused();
        let action = format!("batch:{}:{:?}", wallet_name, &proposal_ids);
        self.verify_owner(&action, &signature, expires_at, nonce);

        assert!(!proposal_ids.is_empty(), "ERR_EMPTY_BATCH");
        assert!(proposal_ids.len() <= 10, "ERR_BATCH_TOO_LARGE: max 10");

        // Pre-validate all proposals before executing any
        for &proposal_id in &proposal_ids {
            let pkey = proposal_key(&wallet_name, proposal_id);
            let proposal = self.proposals.get(&pkey).expect("ERR_PROPOSAL_NOT_FOUND");
            assert!(proposal.status == ProposalStatus::Approved, "ERR_BATCH_NOT_APPROVED: proposal {}", proposal_id);
        }

        let mut executed = 0u32;
        let mut failed = Vec::new();
        for &proposal_id in &proposal_ids {
            let pkey = proposal_key(&wallet_name, proposal_id);
            // Re-check status (previous execution may have side effects)
            if let Some(proposal) = self.proposals.get(&pkey) {
                if proposal.status == ProposalStatus::Approved {
                    self.execute_proposal(&wallet_name, proposal_id);
                    executed += 1;
                } else {
                    failed.push(proposal_id);
                }
            } else {
                failed.push(proposal_id);
            }
        }

        self.emit_event("batch_executed", serde_json::json!({
            "wallet": wallet_name,
            "requested": proposal_ids.len(),
            "executed": executed,
            "failed": failed,
        }));
        log!("Batch executed {}/{} proposals in wallet '{}'", executed, proposal_ids.len(), wallet_name);
    }

    // ── Auto Cleanup ──────────────────────────────────────────────────

    /// Clean up expired proposals to reclaim storage.
    /// Can be called by anyone (not just owner) since it only removes expired proposals.
    /// Reward: caller gets a small storage refund incentive.
    pub fn cleanup_expired(
        &mut self,
        wallet_name: String,
        from_id: u64,
        to_id: u64,
    ) -> u64 {
        self.assert_not_paused();
        let wallet = self.wallet_get_readonly(&wallet_name).expect("ERR_WALLET_NOT_FOUND");
        let now = env::block_timestamp();
        let mut cleaned = 0u64;

        for id in from_id..=to_id.min(wallet.proposal_index.saturating_sub(1)) {
            let pkey = proposal_key(&wallet_name, id);
            if let Some(proposal) = self.proposals.get(&pkey) {
                if (proposal.status == ProposalStatus::Executed ||
                    proposal.status == ProposalStatus::Cancelled) ||
                    (proposal.status == ProposalStatus::Active && proposal.expires_at < now) {
                    // Expired active proposals: cancel them first
                    if proposal.status == ProposalStatus::Active {
                        let ikey = intent_key(&wallet_name, proposal.intent_index);
                        if let Some(mut intent) = self.intents.get(&ikey) {
                            intent.active_proposal_count = intent.active_proposal_count.saturating_sub(1);
                            self.intents.insert(&ikey, &intent);
                        }
                    }
                    self.proposals.remove(&pkey);
                    cleaned += 1;
                }
            }
        }

        if cleaned > 0 {
            self.emit_event("cleanup", serde_json::json!({
                "wallet": wallet_name, "cleaned": cleaned, "from": from_id, "to": to_id,
            }));
            log!("Cleaned up {} expired proposals from wallet '{}'", cleaned, wallet_name);
        }

        cleaned
    }
}

// ── Private Helpers ────────────────────────────────────────────────────────

impl Contract {
    // ── Execution Helpers ─────────────────────────────────────────────

    fn execute_deposit(&mut self, wallet_name: &String) {
        let deposit_amount = env::attached_deposit().as_yoctonear();
        if deposit_amount > 0 {
            self.credit_near(wallet_name, deposit_amount);
        }
        log!("Deposited {} yoctoNEAR to wallet '{}'", deposit_amount, wallet_name);
    }

    fn execute_transfer(&mut self, wallet_name: &String, params: &serde_json::Value) {
        self.assert_not_locked();
        let amount_str = params["amount"].as_str()
            .map(String::from)
            .or_else(|| params["amount"].as_u64().map(|v| v.to_string()))
            .unwrap_or_default();
        let recipient: AccountId = params["recipient"].as_str().expect("ERR_MISSING_RECIPIENT")
            .parse().expect("ERR_INVALID_RECIPIENT");
        let amount: u128 = amount_str.parse().expect("ERR_INVALID_AMOUNT");

        // Enforce daily spend limit
        {
            let mut wallet = self.wallet_get(wallet_name).expect("ERR_WALLET_NOT_FOUND");
            wallet.enforce_spend_limit(env::block_timestamp());
            wallet.track_spend(amount);
            self.wallet_insert(wallet_name, &wallet);
        }

        if let Some(token_str) = params.get("token").and_then(|v| v.as_str()) {
            self.debit_ft(wallet_name, token_str, amount);
            // FT transfer: call ft_transfer on the token contract
            let token_account: AccountId = token_str.parse().expect("ERR_INVALID_TOKEN_ACCOUNT");
            self.locked = true;
            let promise = Promise::new(token_account.clone()).function_call(
                "ft_transfer".to_string(),
                safe_json_ft_transfer(recipient.as_str(), &amount.to_string()),
                NearToken::from_yoctonear(1), // 1 yocto for ft_transfer
                near_sdk::Gas::from_tgas(30),
            );
            // Callback to handle result
            let current_account = env::current_account_id();
            promise.then(Promise::new(current_account).function_call(
                "on_ft_transfer_result".to_string(),
                serde_json::to_vec(&serde_json::json!({
                    "wallet_name": wallet_name,
                    "token": token_account.to_string(),
                    "amount": amount.to_string(),
                    "recipient": recipient.to_string(),
                })).unwrap(),
                NearToken::from_yoctonear(0),
                near_sdk::Gas::from_tgas(5),
            ));
            log!("FT transfer: {} of {} to {} (debit recorded)", amount, token_str, recipient);
        } else {
            self.debit_near(wallet_name, amount);
            Promise::new(recipient.clone()).transfer(NearToken::from_yoctonear(amount));
            log!("Transferred {} yoctoNEAR to {}", amount, recipient);
        }
    }

    fn execute_call(&mut self, wallet_name: &String, params: &serde_json::Value) {
        self.assert_not_locked();
        let receiver_id: AccountId = params["receiver_id"]
            .as_str().expect("ERR_MISSING_RECEIVER_ID")
            .parse().expect("ERR_INVALID_RECEIVER_ID");
        let method_name = params["method_name"]
            .as_str().expect("ERR_MISSING_METHOD_NAME").to_string();
        let args_b64 = params.get("args")
            .and_then(|v| v.as_str()).unwrap_or("");
        let args = base64::engine::general_purpose::STANDARD
            .decode(args_b64).unwrap_or_default();
        let deposit: u128 = params.get("deposit")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let gas_tgas: u64 = params.get("gas")
            .and_then(|v| v.as_u64())
            .unwrap_or(DEFAULT_EXECUTION_GAS_TGAS)
            .min(MAX_EXECUTION_GAS_TGAS);

        // Enforce wallet limits
        {
            let mut wallet = self.wallet_get(wallet_name).expect("ERR_WALLET_NOT_FOUND");

            // Receiver allowlist check
            assert!(
                wallet.is_call_receiver_allowed(&receiver_id),
                "ERR_RECEIVER_NOT_ALLOWED: {} not in allowlist",
                receiver_id
            );

            // Max deposit per call check
            if wallet.call_max_deposit > 0 {
                assert!(
                    deposit <= wallet.call_max_deposit,
                    "ERR_DEPOSIT_EXCEEDS_MAX: {} > {}",
                    deposit, wallet.call_max_deposit
                );
            }

            // Enforce daily spend limit
            wallet.enforce_spend_limit(env::block_timestamp());
            if deposit > 0 {
                wallet.track_spend(deposit);
            }

            // Charge relayer fee (to wallet's storage deposit)
            if wallet.relayer_fee > 0 {
                wallet.storage_deposit = wallet.storage_deposit.saturating_sub(wallet.relayer_fee);
            }

            self.wallet_insert(wallet_name, &wallet);
        }

        // Debit wallet's internal balance for the deposit
        if deposit > 0 {
            self.debit_near(wallet_name, deposit);
        }

        // Execute cross-contract call with callback to handle result
        self.locked = true;
        let prepaid_gas = near_sdk::Gas::from_tgas(gas_tgas);
        let promise = Promise::new(receiver_id.clone()).function_call(
            method_name.clone(),
            args,
            NearToken::from_yoctonear(deposit),
            prepaid_gas,
        );

        // Callback to self to handle result
        let callback_gas = near_sdk::Gas::from_tgas(5);
        let current_account = env::current_account_id();
        promise.then(Promise::new(current_account).function_call(
            "on_call_result".to_string(),
            serde_json::to_vec(&serde_json::json!({
                "wallet_name": wallet_name,
                "receiver_id": receiver_id.to_string(),
                "method_name": method_name,
                "deposit": deposit.to_string(),
            })).unwrap(),
            NearToken::from_yoctonear(0),
            callback_gas,
        ));

        log!("Called {}.{} with {} yoctoNEAR deposit, {} Tgas", receiver_id, method_name, deposit, gas_tgas);
    }

    /// Callback after cross-contract call completes.
    /// If the call failed, refund the deposit back to the wallet.
    /// Only callable by the contract itself (callback).
    pub fn on_call_result(
        &mut self,
        wallet_name: String,
        receiver_id: AccountId,
        method_name: String,
        deposit: u128,
    ) {
        // Only allow self-calls (callback from our own promise)
        assert_eq!(
            env::predecessor_account_id(),
            env::current_account_id(),
            "ERR_CALLBACK_ONLY"
        );
        self.locked = false;
        // Check if the promise succeeded
        let result = env::promise_result(0u64);
        match result {
            PromiseResult::Successful(data) => {
                let data_str = if data.is_empty() {
                    "(empty)".to_string()
                } else {
                    format!("{} bytes", data.len())
                };
                log!("Call {}.{} succeeded: {}", receiver_id, method_name, data_str);
            }
            PromiseResult::Failed => {
                // Refund the deposit on failure
                if deposit > 0 {
                    self.credit_near(&wallet_name, deposit);
                    log!("Call {}.{} FAILED — refunded {} yoctoNEAR to wallet '{}'", receiver_id, method_name, deposit, wallet_name);
                } else {
                    log!("Call {}.{} FAILED (no deposit to refund)", receiver_id, method_name);
                }
                self.emit_event("call_failed", serde_json::json!({
                    "wallet": wallet_name, "receiver": receiver_id, "method": method_name, "refund": deposit,
                }));
            }
        }
    }

    /// Callback after FT transfer. Refunds debit on failure.
    /// Only callable by the contract itself (callback).
    pub fn on_ft_transfer_result(
        &mut self,
        wallet_name: String,
        token: AccountId,
        amount: u128,
        recipient: String,
    ) {
        assert_eq!(
            env::predecessor_account_id(),
            env::current_account_id(),
            "ERR_CALLBACK_ONLY"
        );
        self.locked = false;
        let result = env::promise_result(0u64);
        match result {
            PromiseResult::Successful(_) => {
                log!("FT transfer {} of {} to {} succeeded", amount, token, recipient);
            }
            PromiseResult::Failed => {
                // Refund the FT debit on failure
                self.credit_ft(&wallet_name, token.as_str(), amount);
                log!("FT transfer {} of {} to {} FAILED — refunded to wallet '{}'", amount, token, recipient, wallet_name);
                self.emit_event("ft_transfer_failed", serde_json::json!({
                    "wallet": wallet_name, "token": token.to_string(), "amount": amount.to_string(), "recipient": recipient,
                }));
            }
        }
    }

    fn emit_event(&mut self, event: &str, data: serde_json::Value) {
        self.event_nonce += 1;
        let mut enriched = data;
        // Inject standard fields for indexer convenience
        if let Some(obj) = enriched.as_object_mut() {
            obj.insert("event_nonce".to_string(), serde_json::json!(self.event_nonce));
            obj.insert("block_height".to_string(), serde_json::json!(env::block_height()));
            obj.insert("block_ts".to_string(), serde_json::json!(env::block_timestamp()));
        }
        env::log_str(&format!(
            "EVENT_JSON:{}",
            serde_json::json!({
                "standard": "clear-msig",
                "version": "1.0.0",
                "event": event,
                "nonce": self.event_nonce,
                "data": enriched,
            })
        ));
    }

    /// Shared logic for approve and cancel_vote to avoid duplication.
    /// Shared logic for nostr schnorr approve and cancel_vote.
    fn verify_nostr_approver(
        &mut self,
        wallet_name: String,
        proposal_id: u64,
        approver_index: u16,
        pubkey_hex: String,
        signature: String,
        expires_at: u64,
        action: &str,
    ) {
        let pkey = proposal_key(&wallet_name, proposal_id);
        let mut proposal = self.proposals.get(&pkey).expect("ERR_PROPOSAL_NOT_FOUND");

        assert!(proposal.status == ProposalStatus::Active, "ERR_NOT_ACTIVE");
        assert!(proposal.expires_at > env::block_timestamp(), "ERR_PROPOSAL_EXPIRED");
        assert!(expires_at > env::block_timestamp(), "ERR_SIG_EXPIRED");

        let ikey = intent_key(&wallet_name, proposal.intent_index);
        let intent = self.intents.get(&ikey).expect("ERR_INTENT_NOT_FOUND");
        assert!((approver_index as usize) < intent.nostr_approvers.len(), "ERR_INVALID_NOSTR_APPROVER_INDEX");

        // Verify the pubkey matches the slot
        let expected_pk = &intent.nostr_approvers[approver_index as usize];
        assert_eq!(pubkey_hex, *expected_pk, "ERR_NOSTR_PK_MISMATCH");

        // Build message and verify schnorr signature
        let params: serde_json::Value = serde_json::from_str(&proposal.param_values).unwrap_or_default();
        let msg = message::build_message(&wallet_name, proposal_id, expires_at, action, &intent, &params);

        message::verify_schnorr_signature(&pubkey_hex, &signature, &msg);

        match action {
            "approve" => {
                assert!(!proposal.has_nostr_approved(approver_index as usize), "ERR_NOSTR_ALREADY_APPROVED");
                proposal.set_nostr_approval(approver_index as usize);

                if proposal.approval_count() >= intent.approval_threshold as u32 {
                    proposal.status = ProposalStatus::Approved;
                    proposal.approved_at = env::block_timestamp();

                    self.emit_event("proposal_approved", serde_json::json!({
                        "wallet": wallet_name, "proposal_id": proposal_id,
                        "approval_count": proposal.approval_count(),
                        "nostr": true,
                    }));

                    log!("Proposal #{} approved (nostr)", proposal_id);
                }
            }
            "cancel" => {
                proposal.set_nostr_cancellation(approver_index as usize);

                if proposal.cancellation_count() >= intent.cancellation_threshold as u32 {
                    proposal.status = ProposalStatus::Cancelled;
                    let mut intent_mut = intent.clone();
                    intent_mut.active_proposal_count = intent_mut.active_proposal_count.saturating_sub(1);
                    self.intents.insert(&ikey, &intent_mut);

                    self.emit_event("proposal_cancelled", serde_json::json!({
                        "wallet": wallet_name, "proposal_id": proposal_id,
                        "cancellation_count": proposal.cancellation_count(),
                        "nostr": true,
                    }));
                }
            }
            _ => env::panic_str("ERR_INVALID_ACTION"),
        }

        self.proposals.insert(&pkey, &proposal);
    }

    fn create_meta_intents(&mut self, name: &str, owner: &AccountId) {
        let owner_npub = self.owner_npubs.first().cloned().unwrap_or_default();
        let make = |index: u32, itype: IntentType, iname: &str, template: &str, params: Vec<ParamDef>| Intent {
            wallet_name: name.to_string(),
            index,
            intent_type: itype,
            name: iname.to_string(),
            template: template.to_string(),
            proposers: vec![owner.clone()],
            approvers: vec![owner.clone()],
            nostr_approvers: vec![owner_npub.clone()],
            approval_threshold: 1,
            cancellation_threshold: 1,
            timelock_seconds: 0,
            params,
            execution_gas_tgas: DEFAULT_EXECUTION_GAS_TGAS,
            active: true,
            active_proposal_count: 0,
        };

        self.intents.insert(&intent_key(name, 0), &make(
            0, IntentType::AddIntent, "AddIntent", "add intent definition_hash: {hash}",
            vec![ParamDef { name: "hash".to_string(), param_type: ParamType::String, max_value: None }],
        ));
        self.intents.insert(&intent_key(name, 1), &make(
            1, IntentType::RemoveIntent, "RemoveIntent", "remove intent {index}",
            vec![ParamDef { name: "index".to_string(), param_type: ParamType::U64, max_value: None }],
        ));
        self.intents.insert(&intent_key(name, 2), &make(
            2, IntentType::UpdateIntent, "UpdateIntent", "update intent {index}",
            vec![ParamDef { name: "index".to_string(), param_type: ParamType::U64, max_value: None }],
        ));
    }

    fn validate_params(&self, intent: &Intent, params: &serde_json::Value) {
        for pd in &intent.params {
            match params.get(&pd.name) {
                None => env::panic_str(&format!("ERR_MISSING_PARAM: {}", pd.name)),
                Some(val) => match pd.param_type {
                    ParamType::AccountId => {
                        let s = val.as_str().unwrap_or_else(|| env::panic_str(&format!("ERR_EXPECTED_STRING: {}", pd.name)));
                        s.parse::<AccountId>().unwrap_or_else(|_| env::panic_str(&format!("ERR_INVALID_ACCOUNT: {}", pd.name)));
                    }
                    ParamType::U64 => {
                        let v = val.as_u64()
                            .or_else(|| val.as_str().and_then(|s| s.parse::<u64>().ok()))
                            .unwrap_or_else(|| env::panic_str(&format!("ERR_EXPECTED_U64: {}", pd.name)));
                        if let Some(max) = &pd.max_value {
                            assert!((v as u128) <= max.0, "ERR_EXCEEDS_MAX: {}", pd.name);
                        }
                    }
                    ParamType::U128 => {
                        let s = match val {
                            serde_json::Value::String(s) => s.clone(),
                            serde_json::Value::Number(n) => n.to_string(),
                            _ => env::panic_str(&format!("ERR_EXPECTED_U128: {}", pd.name)),
                        };
                        let v: u128 = s.parse().unwrap_or_else(|_| env::panic_str(&format!("ERR_INVALID_U128: {}", pd.name)));
                        if let Some(max) = &pd.max_value {
                            assert!(v <= max.0, "ERR_EXCEEDS_MAX: {}", pd.name);
                        }
                    }
                    ParamType::String => {
                        val.as_str().unwrap_or_else(|| env::panic_str(&format!("ERR_EXPECTED_STRING: {}", pd.name)));
                    }
                    ParamType::Bool => {
                        val.as_bool().unwrap_or_else(|| env::panic_str(&format!("ERR_EXPECTED_BOOL: {}", pd.name)));
                    }
                },
            }
        }
    }
}

#[cfg(test)]
mod bip340_tests;

#[cfg(test)]
// mod verification;
#[cfg(test)]
mod integration_tests;
#[cfg(test)]
mod vm_tests;

#[cfg(test)]
mod tests {
    use super::*;

    // ── migrate() guard tests (Tier-1 hardening) ─────────────────────────
    //
    // Seeds the mock VM STATE key directly with borsh-serialized state and
    // proves: (1) v3 state migrates to v5 with empty session registries,
    // (2) re-running migrate() against the resulting v5 state panics
    //     (trailing-bytes deserialize failure → ERR_NO_PREVIOUS_STATE),
    // (3) a v5-shaped state that *would* deserialize as ContractV3 (forced
    //     via a version-3 first field, prefix-compatible layout scenario)
    //     is rejected by the version assert → ERR_NOT_V3_STATE.
    mod migrate_guard {
        use super::super::*;
        use near_sdk::borsh::BorshSerialize;
        use near_sdk::{testing_env, test_utils::VMContextBuilder};

        fn init_test_env() {
            let ctx = VMContextBuilder::new().build();
            testing_env!(ctx);
        }

        fn seed_state<T: BorshSerialize>(state: &T) {
            let bytes = near_sdk::borsh::to_vec(state).expect("serialize state");
            near_sdk::env::storage_write(b"STATE", &bytes);
        }

        fn make_v3() -> ContractV3 {
            ContractV3 {
                version: 3,
                owner_npubs: vec!["6a04ab98d9e4774ad806e302dddeb63bea16b5cb5f223ee77478e861bb583eb3".into()],
                guardian_npub: None,
                wallets: LookupMap::new(StorageKey::Wallets),
                intents: LookupMap::new(StorageKey::Intents),
                proposals: LookupMap::new(StorageKey::Proposals),
                delegations: LookupMap::new(StorageKey::Delegations),
                event_nonce: 7,
                owner_nonce: 42,
                owner_nonce_bitmap: 0,
                wallet_names: Vector::new(StorageKey::WalletNames),
                paused: false,
                locked: false,
            }
        }

        #[test]
        fn migrate_v3_to_v5_preserves_state_and_empties_sessions() {
            init_test_env();
            seed_state(&make_v3());

            let migrated = Contract::migrate();
            assert_eq!(migrated.version, 5, "migrated contract must be v5");
            assert_eq!(migrated.event_nonce, 7, "state fields carried over");
            assert_eq!(migrated.owner_nonce, 42, "nonce window carried over");
            assert!(
                migrated.session_keys.is_empty(),
                "session registries start empty"
            );
        }

        #[test]
        fn remigrate_against_v5_state_panics() {
            init_test_env();
            // Realistic scenario: v5 state on chain (post-migration), someone
            // calls migrate() again. The v5 bytes contain trailing session
            // registry serialization that cannot deserialize as ContractV3.
            seed_state(&make_v3());
            let migrated = Contract::migrate();
            seed_state(&migrated); // v5-shaped state now under STATE

            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                Contract::migrate();
            }));
            assert!(result.is_err(), "re-migration must panic");
        }

        #[test]
        fn remigrate_prefix_compatible_v5_state_blocked_by_version_assert() {
            init_test_env();
            // Adversarial layout scenario: future state that happens to
            // deserialize cleanly as ContractV3 (no trailing bytes) but is
            // NOT v3 — version field differs → assert fires, state untouched.
            let mut fake = make_v3();
            fake.version = 5; // pretend current state reports v5
            seed_state(&fake);

            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                Contract::migrate();
            }));
            assert!(result.is_err(), "version-5 state must not re-migrate");
            // And prove the guard specifically: v4-shaped prefix state too.
            let mut fake4 = make_v3();
            fake4.version = 4;
            seed_state(&fake4);
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                Contract::migrate();
            }));
            assert!(result.is_err(), "version-4 state must not re-migrate");
        }
    }

    #[test]
    fn test_normalize_pk_hex() {
        let hex64 = "ab".repeat(32);
        // bare hex
        assert_eq!(normalize_pk_hex(&hex64), hex64);
        // with prefix (case-insensitive)
        assert_eq!(normalize_pk_hex(&format!("ed25519:{}", hex64.to_uppercase())), hex64);
        // wrong length
        let r = std::panic::catch_unwind(|| normalize_pk_hex("abcd"));
        assert!(r.is_err());
        // non-hex
        let r = std::panic::catch_unwind(|| normalize_pk_hex(&"zz".repeat(32)));
        assert!(r.is_err());
    }

    #[test]
    fn test_session_meta_expiry() {
        let meta = SessionMeta {
            public_key: "ab".repeat(32),
            wallet: "w".to_string(),
            created_at: 100,
            expires_at: 1_000,
            funded_yocto: 0,
            label: None,
        };
        assert!(meta.is_active(999));
        assert!(!meta.is_active(1_000), "session must be dead at exactly expires_at");
        assert!(!meta.is_active(2_000));
    }

    #[test]
    fn test_session_funding_cap_arithmetic() {
        // cumulative funding must stay within MAX_SESSION_FUND_YOCTO (1 NEAR)
        let mut funded: u128 = 0;
        for amt in [300_000_000_000_000_000_000_000u128, 400_000_000_000_000_000_000_000] {
            assert!(funded + amt <= MAX_SESSION_FUND_YOCTO);
            funded += amt;
        }
        // one more yocto over the cap must be rejected
        assert!(funded + 300_000_000_000_000_000_000_001 > MAX_SESSION_FUND_YOCTO);
    }

    #[test]
    fn test_session_auth_rejections() {
        use near_sdk::test_utils::VMContextBuilder;
        use std::panic::{catch_unwind, AssertUnwindSafe};

        fn panic_msg<T>(r: &Result<T, Box<dyn std::any::Any + Send>>) -> String {
            match r {
                Ok(_) => "NO PANIC".to_string(),
                Err(e) => e
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
                    .unwrap_or_else(|| "(panic with () payload — env::panic_str)".to_string()),
            }
        }

        let owner_npub = "11".repeat(32);
        let pk_hex = "ab".repeat(32);
        let pk = parse_ed25519_pk(&pk_hex);

        let mut b = VMContextBuilder::new();
        b.signer_account_pk(pk);
        b.block_timestamp(1_000_000_000_000_000_000);
        near_sdk::testing_env!(b.build());

        let mut c = Contract::new(vec![owner_npub]);

        // 1) unregistered signer key -> rejected (ERR_NOT_SESSION_KEY via panic_str)
        let r = catch_unwind(AssertUnwindSafe(|| c.session_ping()));
        assert!(r.is_err(), "unregistered key must be rejected");

        // 2) expired session -> ERR_SESSION_EXPIRED
        c.sessions.insert(
            &pk_hex,
            &SessionMeta {
                public_key: pk_hex.clone(),
                wallet: "w1".to_string(),
                created_at: 0,
                expires_at: 999_999_999_999_999_999,
                funded_yocto: 0,
                label: None,
            },
        );
        let r = catch_unwind(AssertUnwindSafe(|| c.session_ping()));
        assert!(panic_msg(&r).contains("ERR_SESSION_EXPIRED"), "got: {}", panic_msg(&r));

        // 3) active session -> ping works and identifies wallet + key
        c.sessions.insert(
            &pk_hex,
            &SessionMeta {
                public_key: pk_hex.clone(),
                wallet: "w1".to_string(),
                created_at: 0,
                expires_at: 2_000_000_000_000_000_000,
                funded_yocto: 0,
                label: None,
            },
        );
        let pong = c.session_ping();
        assert!(pong.starts_with("pong:w1:"), "got: {pong}");

        // 4) session scoped to a different wallet -> ERR_SESSION_WALLET_MISMATCH
        let r = catch_unwind(AssertUnwindSafe(|| {
            c.submit_action(
                "propose".to_string(),
                "w2".to_string(),
                0,
                0,
                "{}".to_string(),
                3_000_000_000_000_000_000,
                0,
                "00".to_string(),
            )
        }));
        assert!(
            panic_msg(&r).contains("ERR_SESSION_WALLET_MISMATCH"),
            "got: {}",
            panic_msg(&r)
        );

        // 5) paused contract -> ERR_CONTRACT_PAUSED even for valid session
        c.paused = true;
        let r = catch_unwind(AssertUnwindSafe(|| c.session_ping()));
        assert!(panic_msg(&r).contains("ERR_CONTRACT_PAUSED"), "got: {}", panic_msg(&r));
    }

    #[test]
    fn test_parse_ed25519_pk_roundtrip() {
        use near_sdk::test_utils::VMContextBuilder;
        let hex64 = "cd".repeat(32);
        near_sdk::testing_env!(VMContextBuilder::new().build());
        let parsed = parse_ed25519_pk(&hex64);
        let bytes = parsed.into_bytes();
        assert_eq!(bytes[0], 0u8, "curve prefix byte");
        assert_eq!(hex_encode(&bytes[1..]), hex64);
    }

    #[test]
    fn test_render_template() {
        let intent = Intent {
            wallet_name: "test".to_string(),
            index: 3,
            intent_type: IntentType::Custom,
            name: "Transfer NEAR".to_string(),
            template: "transfer {amount} yoctoNEAR to {recipient}".to_string(),
            proposers: vec![],
            approvers: vec![],
            nostr_approvers: vec![],
            approval_threshold: 1,
            cancellation_threshold: 1,
            timelock_seconds: 0,
            params: vec![
                ParamDef { name: "amount".to_string(), param_type: ParamType::U128, max_value: None },
                ParamDef { name: "recipient".to_string(), param_type: ParamType::AccountId, max_value: None },
            ],
            execution_gas_tgas: 50,
            active: true,
            active_proposal_count: 0,
        };

        let params = serde_json::json!({
            "amount": "1000000000000000000000000",
            "recipient": "bob.near"
        });

        assert_eq!(
            intent.render_template(&params),
            "transfer 1000000000000000000000000 yoctoNEAR to bob.near"
        );
    }

    #[test]
    fn test_proposal_bitmap() {
        let mut p = Proposal {
            id: 0, wallet_name: "w".to_string(), intent_index: 0,
            proposer: "alice.near".parse().unwrap(), status: ProposalStatus::Active,
            proposed_at: 0, approved_at: 0, expires_at: 0,
            approval_bitmap: 0, cancellation_bitmap: 0,
            nostr_approval_bitmap: 0, nostr_cancellation_bitmap: 0,
            param_values: "{}".to_string(), message: "".to_string(),
            intent_params_hash: "".to_string(),
        };

        assert_eq!(p.approval_count(), 0);
        assert!(!p.has_approved(0));

        p.set_approval(0);
        assert!(p.has_approved(0));
        assert_eq!(p.approval_count(), 1);

        p.set_cancellation(0);
        assert!(!p.has_approved(0)); // cancelled clears approval
        assert_eq!(p.cancellation_count(), 1);

        p.reset_votes();
        assert_eq!(p.approval_count(), 0);
        assert_eq!(p.cancellation_count(), 0);
    }

    #[test]
    fn test_template_injection_blocked() {
        let intent = Intent {
            wallet_name: "test".to_string(),
            index: 0,
            intent_type: IntentType::Custom,
            name: "test".to_string(),
            template: "do {param}".to_string(),
            proposers: vec![], approvers: vec![], nostr_approvers: vec![],
            approval_threshold: 1, cancellation_threshold: 1,
            timelock_seconds: 0,
            params: vec![ParamDef { name: "param".to_string(), param_type: ParamType::String, max_value: None }],
            execution_gas_tgas: 50,
            active: true,
            active_proposal_count: 0,
        };

        // Pipe should be rejected
        let params = serde_json::json!({ "param": "evil | wallet: fake" });
        let result = std::panic::catch_unwind(|| intent.render_template(&params));
        assert!(result.is_err());
    }

    #[test]
    fn test_safe_json_ft_transfer() {
        let json = safe_json_ft_transfer("bob.near", "1000000");
        let parsed: serde_json::Value = serde_json::from_slice(&json).unwrap();
        assert_eq!(parsed["receiver_id"], "bob.near");
        assert_eq!(parsed["amount"], "1000000");
        assert_eq!(parsed["msg"], "");
    }
}

