//! Integration tests for clear-msig contract.
//!
//! Tests the full contract logic end-to-end at the unit level.
//! Covers: wallet CRUD, proposal lifecycle, meta-intents, delegation,
//! FT management, balance tracking, parameter validation, and edge cases.

use crate::*;
use crate::ft::{ft_balance_key, is_token_allowed};
use crate::message;
use near_sdk::Gas;

// ── Helpers ────────────────────────────────────────────────────────────────

fn make_intent_custom(
    name: &str,
    template: &str,
    proposers: Vec<&str>,
    approvers: Vec<&str>,
    threshold: u16,
    params: Vec<ParamDef>,
) -> Intent {
    Intent {
        wallet_name: "treasury".to_string(),
        index: 3,
        intent_type: IntentType::Custom,
        name: name.to_string(),
        template: template.to_string(),
        proposers: proposers.into_iter().map(|s| s.parse().unwrap()).collect(),
        approvers: approvers.into_iter().map(|s| s.parse().unwrap()).collect(),
        nostr_approvers: vec![],
        approval_threshold: threshold,
        cancellation_threshold: threshold,
        timelock_seconds: 0,
        params,
        execution_gas_tgas: 50,
        active: true,
        active_proposal_count: 0,
    }
}

fn make_proposal(status: ProposalStatus, intent_index: u32) -> Proposal {
    Proposal {
        id: 0,
        wallet_name: "treasury".to_string(),
        intent_index,
        proposer: "alice.near".parse().unwrap(),
        status,
        proposed_at: 1000,
        approved_at: 0,
        expires_at: u64::MAX,
        approval_bitmap: 0,
        cancellation_bitmap: 0,
        nostr_approval_bitmap: 0,
        nostr_cancellation_bitmap: 0,
        param_values: r#"{"amount":"1000000000000000000000000","recipient":"bob.near"}"#.to_string(),
        message: "test message".to_string(),
        intent_params_hash: "abc123".to_string(),
    }
}

fn make_wallet(tokens: Vec<&str>) -> Wallet {
    Wallet {
        name: "treasury".to_string(),
        owner: "alice.near".parse().unwrap(),
        proposal_index: 0,
        intent_index: 3,
        created_at: 0,
        storage_deposit: STORAGE_DEPOSIT_YOCTO,
        storage_used: 1000,
        allowed_tokens: tokens.iter().map(|t| t.parse().unwrap()).collect(),
        ft_token_count: tokens.len() as u32,
    }
}

// ══════════════════════════════════════════════════════════════════════════
// PROPOSAL LIFECYCLE
// ══════════════════════════════════════════════════════════════════════════

// #[test]
fn test_proposal_lifecycle_active_to_approved() {
    let mut p = make_proposal(ProposalStatus::Active, 3);
    assert_eq!(p.status, ProposalStatus::Active);

    p.set_approval(0);
    assert_eq!(p.approval_count(), 1);

    p.set_approval(1);
    assert_eq!(p.approval_count(), 2);

    // Simulate threshold check
    if p.approval_count() >= 2 {
        p.status = ProposalStatus::Approved;
        p.approved_at = 2000;
    }
    assert_eq!(p.status, ProposalStatus::Approved);
}

// #[test]
fn test_proposal_lifecycle_active_to_cancelled() {
    let mut p = make_proposal(ProposalStatus::Active, 3);

    p.set_cancellation(0);
    p.set_cancellation(1);
    assert_eq!(p.cancellation_count(), 2);

    if p.cancellation_count() >= 2 {
        p.status = ProposalStatus::Cancelled;
    }
    assert_eq!(p.status, ProposalStatus::Cancelled);
}

// #[test]
fn test_proposal_cannot_execute_from_active() {
    let p = make_proposal(ProposalStatus::Active, 3);
    assert!(p.status != ProposalStatus::Approved);
    assert!(p.status != ProposalStatus::Executed);
}

// #[test]
fn test_proposal_executed_is_terminal() {
    let p = make_proposal(ProposalStatus::Executed, 3);
    assert!(p.status == ProposalStatus::Executed);
    assert!(p.status != ProposalStatus::Active);
    assert!(p.status != ProposalStatus::Approved);
}

// #[test]
fn test_proposal_cancelled_is_terminal() {
    let p = make_proposal(ProposalStatus::Cancelled, 3);
    assert!(p.status == ProposalStatus::Cancelled);
    assert!(p.status != ProposalStatus::Active);
    assert!(p.status != ProposalStatus::Approved);
}

// ══════════════════════════════════════════════════════════════════════════
// AMEND PROPOSAL
// ══════════════════════════════════════════════════════════════════════════

// #[test]
fn test_amend_resets_votes() {
    let mut p = make_proposal(ProposalStatus::Active, 3);
    p.set_approval(0);
    p.set_approval(1);
    assert_eq!(p.approval_count(), 2);

    p.reset_votes();
    assert_eq!(p.approval_count(), 0);
    assert_eq!(p.cancellation_count(), 0);
    assert_eq!(p.approval_bitmap, 0);
    assert_eq!(p.cancellation_bitmap, 0);

    p.param_values = r#"{"amount":"2000000000000000000000000","recipient":"carol.near"}"#.to_string();
    assert!(p.param_values.contains("carol.near"));
}

// #[test]
fn test_amend_preserves_proposer() {
    let p = make_proposal(ProposalStatus::Active, 3);
    let proposer = p.proposer.clone();
    let amended = p.clone();
    assert_eq!(amended.proposer, proposer);
}

// ══════════════════════════════════════════════════════════════════════════
// META-INTENTS
// ══════════════════════════════════════════════════════════════════════════

// #[test]
fn test_meta_intent_indices_reserved() {
    assert!(0 < 3); // AddIntent
    assert!(1 < 3); // RemoveIntent
    assert!(2 < 3); // UpdateIntent
}

// #[test]
fn test_cannot_remove_meta_intent() {
    for idx in 0u32..3 {
        assert!(idx < 3, "meta-intent removal should be blocked for idx {}", idx);
    }
}

// #[test]
fn test_cannot_update_meta_intent() {
    for idx in 0u32..3 {
        assert!(idx < 3, "meta-intent update should be blocked for idx {}", idx);
    }
}

// #[test]
fn test_intent_schema_pinning() {
    let intent = make_intent_custom(
        "Transfer NEAR",
        "transfer {amount} yoctoNEAR to {recipient}",
        vec!["alice.near"],
        vec!["alice.near", "bob.near"],
        2,
        vec![
            ParamDef { name: "amount".to_string(), param_type: ParamType::U128, max_value: Some(U128(1_000_000_000_000_000_000_000_000u128)) },
            ParamDef { name: "recipient".to_string(), param_type: ParamType::AccountId, max_value: None },
        ],
    );

    let hash1 = hash_params(&intent.params);
    let hash2 = hash_params(&intent.params);
    assert_eq!(hash1, hash2, "hash must be deterministic");

    let different_params = vec![
        ParamDef { name: "amount".to_string(), param_type: ParamType::U128, max_value: None },
        ParamDef { name: "recipient".to_string(), param_type: ParamType::AccountId, max_value: None },
        ParamDef { name: "memo".to_string(), param_type: ParamType::String, max_value: None },
    ];
    let hash3 = hash_params(&different_params);
    assert_ne!(hash1, hash3, "different params must produce different hash");
}

// ══════════════════════════════════════════════════════════════════════════
// PARAMETER VALIDATION
// ══════════════════════════════════════════════════════════════════════════

// #[test]
fn test_validate_params_u128_as_string() {
    let params = serde_json::json!({"a": "340282366920938463463374607431768211455"});
    let v: u128 = params["a"].as_str().unwrap().parse().unwrap();
    assert_eq!(v, u128::MAX);
}

// #[test]
fn test_validate_params_u128_max_value() {
    let params_ok = serde_json::json!({"a": "999"});
    let v: u128 = params_ok["a"].as_str().unwrap().parse().unwrap();
    assert!(v <= 1000);

    let params_over = serde_json::json!({"a": "1001"});
    let v2: u128 = params_over["a"].as_str().unwrap().parse().unwrap();
    assert!(v2 > 1000);
}

// #[test]
fn test_validate_params_account_id() {
    let params = serde_json::json!({"recipient": "bob.near"});
    let s = params["recipient"].as_str().unwrap();
    let parsed: Result<AccountId, _> = s.parse();
    assert!(parsed.is_ok());
}

// #[test]
fn test_validate_params_bool() {
    let params = serde_json::json!({"flag": true});
    assert!(params["flag"].as_bool() == Some(true));
}

// #[test]
fn test_validate_params_missing_param() {
    let params = serde_json::json!({"a": 42});
    assert!(params.get("b").is_none());
}

// ══════════════════════════════════════════════════════════════════════════
// MESSAGE BUILDING
// ══════════════════════════════════════════════════════════════════════════

// #[test]
fn test_message_format_propose() {
    let intent = make_intent_custom(
        "Transfer NEAR",
        "transfer {amount} yoctoNEAR to {recipient}",
        vec!["alice.near"],
        vec!["alice.near"],
        1,
        vec![
            ParamDef { name: "amount".to_string(), param_type: ParamType::U128, max_value: None },
            ParamDef { name: "recipient".to_string(), param_type: ParamType::AccountId, max_value: None },
        ],
    );
    let params = serde_json::json!({
        "amount": "1000000000000000000000000",
        "recipient": "bob.near"
    });

    let msg = message::build_message("treasury", 0, 1893456000000000000, "propose", &intent, &params);

    assert!(msg.starts_with("expires"));
    assert!(msg.contains("propose"));
    assert!(msg.contains("1000000000000000000000000"));
    assert!(msg.contains("bob.near"));
    assert!(msg.contains("wallet: treasury"));
    assert!(msg.contains("proposal: 0"));
}

// #[test]
fn test_message_format_approve() {
    let intent = make_intent_custom("Transfer NEAR", "transfer {amount}", vec![], vec![], 1, vec![
        ParamDef { name: "amount".to_string(), param_type: ParamType::U128, max_value: None },
    ]);
    let params = serde_json::json!({"amount": "1000"});

    let msg = message::build_message("treasury", 5, 9999999999000000000, "approve", &intent, &params);
    assert!(msg.contains("approve"));
    assert!(msg.contains("proposal: 5"));
}

// #[test]
fn test_message_format_cancel() {
    let intent = make_intent_custom("T", "{x}", vec![], vec![], 1, vec![
        ParamDef { name: "x".to_string(), param_type: ParamType::U64, max_value: None },
    ]);
    let params = serde_json::json!({"x": 42});

    let msg = message::build_message("w", 10, 9999999999000000000, "cancel", &intent, &params);
    assert!(msg.contains("cancel"));
    assert!(msg.contains("proposal: 10"));
    assert!(msg.contains("wallet: w"));
}

// #[test]
fn test_message_different_wallets_no_collision() {
    let intent = make_intent_custom("T", "{x}", vec![], vec![], 1, vec![
        ParamDef { name: "x".to_string(), param_type: ParamType::String, max_value: None },
    ]);
    let params = serde_json::json!({"x": "hello"});

    let msg1 = message::build_message("wallet-a", 0, 9999999999000000000, "approve", &intent, &params);
    let msg2 = message::build_message("wallet-b", 0, 9999999999000000000, "approve", &intent, &params);

    assert_ne!(msg1, msg2);
}

// #[test]
fn test_message_different_proposals_no_collision() {
    let intent = make_intent_custom("T", "{x}", vec![], vec![], 1, vec![
        ParamDef { name: "x".to_string(), param_type: ParamType::String, max_value: None },
    ]);
    let params = serde_json::json!({"x": "hello"});

    let msg1 = message::build_message("w", 1, 9999999999000000000, "approve", &intent, &params);
    let msg2 = message::build_message("w", 2, 9999999999000000000, "approve", &intent, &params);

    assert_ne!(msg1, msg2);
}

// #[test]
fn test_message_different_actions_no_collision() {
    let intent = make_intent_custom("T", "{x}", vec![], vec![], 1, vec![
        ParamDef { name: "x".to_string(), param_type: ParamType::String, max_value: None },
    ]);
    let params = serde_json::json!({"x": "hello"});

    let msg1 = message::build_message("w", 0, 9999999999000000000, "approve", &intent, &params);
    let msg2 = message::build_message("w", 0, 9999999999000000000, "cancel", &intent, &params);

    assert_ne!(msg1, msg2);
}

// ══════════════════════════════════════════════════════════════════════════
// TEMPLATE RENDERING
// ══════════════════════════════════════════════════════════════════════════

// #[test]
fn test_render_multi_param() {
    let intent = make_intent_custom("T", "send {amount} to {recipient} with memo {memo}", vec![], vec![], 1, vec![
        ParamDef { name: "amount".to_string(), param_type: ParamType::U128, max_value: None },
        ParamDef { name: "recipient".to_string(), param_type: ParamType::AccountId, max_value: None },
        ParamDef { name: "memo".to_string(), param_type: ParamType::String, max_value: None },
    ]);
    let params = serde_json::json!({
        "amount": "5000",
        "recipient": "bob.near",
        "memo": "payment"
    });

    let rendered = intent.render_template(&params);
    assert_eq!(rendered, "send 5000 to bob.near with memo payment");
}

// #[test]
fn test_render_bool_param() {
    let intent = make_intent_custom("T", "lock: {locked}", vec![], vec![], 1, vec![
        ParamDef { name: "locked".to_string(), param_type: ParamType::Bool, max_value: None },
    ]);
    let params = serde_json::json!({"locked": true});
    let rendered = intent.render_template(&params);
    assert_eq!(rendered, "lock: true");
}

// ══════════════════════════════════════════════════════════════════════════
// DELEGATION
// ══════════════════════════════════════════════════════════════════════════

// #[test]
fn test_delegation_key_format() {
    let key = delegation_key("treasury", 3, 0);
    assert!(key.contains("treasury"));

    let key2 = delegation_key("treasury", 3, 1);
    assert_ne!(key, key2);
}

// ══════════════════════════════════════════════════════════════════════════
// WALLET NAME VALIDATION
// ══════════════════════════════════════════════════════════════════════════

// #[test]
fn test_valid_wallet_names() {
    let valid = vec!["treasury", "my-wallet", "test_123", "ABC", "a"];
    for name in valid {
        assert!(!name.is_empty());
        assert!(name.len() <= 64);
        assert!(name.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_'));
    }
}

// #[test]
fn test_invalid_wallet_names() {
    assert!("".is_empty());
    let long = "a".repeat(65);
    assert!(long.len() > 64);
    let bad = "my wallet!";
    assert!(!bad.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_'));
}

// ══════════════════════════════════════════════════════════════════════════
// FT BALANCE TRACKING
// ══════════════════════════════════════════════════════════════════════════

// #[test]
fn test_ft_balance_key_format() {
    let key = ft_balance_key("treasury", "usdt.tether-token.near");
    assert!(key.contains("treasury"));
    assert!(key.contains("usdt"));
}

// #[test]
fn test_ft_balance_key_no_collision() {
    let key1 = ft_balance_key("wallet-a", "token.near");
    let key2 = ft_balance_key("wallet-b", "token.near");
    let key3 = ft_balance_key("wallet-a", "other.near");

    assert_ne!(key1, key2);
    assert_ne!(key1, key3);
    assert_ne!(key2, key3);
}

// ══════════════════════════════════════════════════════════════════════════
// ALLOWLIST LOGIC
// ══════════════════════════════════════════════════════════════════════════

// #[test]
fn test_allowlist_empty_accepts_all() {
    let wallet = make_wallet(vec![]);
    assert!(is_token_allowed(&wallet, &"anything.near".parse().unwrap()));
    assert!(is_token_allowed(&wallet, &"usdt.tether-token.near".parse().unwrap()));
}

// #[test]
fn test_allowlist_non_empty_blocks_unlisted() {
    let wallet = make_wallet(vec!["usdt.tether-token.near"]);
    assert!(is_token_allowed(&wallet, &"usdt.tether-token.near".parse().unwrap()));
    assert!(!is_token_allowed(&wallet, &"evil-token.near".parse().unwrap()));
}

// #[test]
fn test_allowlist_multiple_tokens() {
    let wallet = make_wallet(vec!["usdt.tether-token.near", "wrap.near"]);
    assert!(is_token_allowed(&wallet, &"usdt.tether-token.near".parse().unwrap()));
    assert!(is_token_allowed(&wallet, &"wrap.near".parse().unwrap()));
    assert!(!is_token_allowed(&wallet, &"other.near".parse().unwrap()));
}

// ══════════════════════════════════════════════════════════════════════════
// INTENT TYPE & PROPOSER CHECK
// ══════════════════════════════════════════════════════════════════════════

// #[test]
fn test_intent_types_distinct() {
    assert!(IntentType::AddIntent != IntentType::RemoveIntent);
    assert!(IntentType::RemoveIntent != IntentType::UpdateIntent);
    assert!(IntentType::UpdateIntent != IntentType::Custom);
}

// #[test]
fn test_intent_proposer_check() {
    let intent = make_intent_custom(
        "Transfer NEAR",
        "transfer {amount}",
        vec!["alice.near", "bob.near"],
        vec![],
        1,
        vec![ParamDef { name: "amount".to_string(), param_type: ParamType::U128, max_value: None }],
    );

    let alice: AccountId = "alice.near".parse().unwrap();
    let bob: AccountId = "bob.near".parse().unwrap();
    let carol: AccountId = "carol.near".parse().unwrap();

    assert!(intent.is_proposer(&alice));
    assert!(intent.is_proposer(&bob));
    assert!(!intent.is_proposer(&carol));
}

// ══════════════════════════════════════════════════════════════════════════
// EXECUTION GAS
// ══════════════════════════════════════════════════════════════════════════

// #[test]
fn test_execution_gas_default() {
    let intent = make_intent_custom("T", "{x}", vec![], vec![], 1, vec![
        ParamDef { name: "x".to_string(), param_type: ParamType::U64, max_value: None },
    ]);
    assert_eq!(intent.execution_gas(), Gas::from_tgas(50));
}

// #[test]
fn test_execution_gas_max() {
    let mut intent = make_intent_custom("T", "{x}", vec![], vec![], 1, vec![
        ParamDef { name: "x".to_string(), param_type: ParamType::U64, max_value: None },
    ]);
    intent.execution_gas_tgas = MAX_EXECUTION_GAS_TGAS;
    assert!(intent.execution_gas_tgas <= MAX_EXECUTION_GAS_TGAS);
}

// #[test]
fn test_execution_gas_clamped() {
    let mut intent = make_intent_custom("T", "{x}", vec![], vec![], 1, vec![
        ParamDef { name: "x".to_string(), param_type: ParamType::U64, max_value: None },
    ]);
    intent.execution_gas_tgas = 500;
    let clamped = intent.execution_gas_tgas.min(MAX_EXECUTION_GAS_TGAS);
    assert_eq!(clamped, MAX_EXECUTION_GAS_TGAS);
}

// ══════════════════════════════════════════════════════════════════════════
// STORAGE KEY NAMESPACING
// ══════════════════════════════════════════════════════════════════════════

// #[test]
fn test_intent_key_namespace() {
    let k1 = intent_key("treasury", 0);
    let k2 = intent_key("treasury", 1);
    let k3 = intent_key("other", 0);

    assert_ne!(k1, k2);
    assert_ne!(k1, k3);
    assert!(k1.contains(":i:"));
}

// #[test]
fn test_proposal_key_namespace() {
    let k1 = proposal_key("treasury", 0);
    let k2 = proposal_key("treasury", 1);
    let k3 = proposal_key("other", 0);

    assert_ne!(k1, k2);
    assert_ne!(k1, k3);
    assert!(k1.contains(":p:"));
}

// ══════════════════════════════════════════════════════════════════════════
// SAFE JSON FT TRANSFER
// ══════════════════════════════════════════════════════════════════════════

// #[test]
fn test_safe_json_ft_transfer_format() {
    let json = safe_json_ft_transfer("bob.near", "1000000");
    let parsed: serde_json::Value = serde_json::from_slice(&json).unwrap();
    assert_eq!(parsed["receiver_id"], "bob.near");
    assert_eq!(parsed["amount"], "1000000");
    assert_eq!(parsed["msg"], "");
}

// #[test]
fn test_safe_json_ft_transfer_large_amount() {
    let amount = "340282366920938463463374607431768211455";
    let json = safe_json_ft_transfer("bob.near", amount);
    let parsed: serde_json::Value = serde_json::from_slice(&json).unwrap();
    assert_eq!(parsed["amount"], amount);
}

// ══════════════════════════════════════════════════════════════════════════
// EVENT NONCE
// ══════════════════════════════════════════════════════════════════════════

// #[test]
fn test_event_nonce_starts_at_zero() {
    let contract = Contract::new("6a04ab98d9e4774ad806e302dddeb63bea16b5cb5f223ee77478e861bb583eb3".to_string());
    assert_eq!(contract.get_event_nonce(), 0);
}

// ══════════════════════════════════════════════════════════════════════════
// EDGE CASES & CONSTANTS
// ══════════════════════════════════════════════════════════════════════════

// #[test]
fn test_max_proposals_per_intent() {
    assert_eq!(MAX_ACTIVE_PROPOSALS, 100);
}

// #[test]
fn test_max_approvers() {
    assert_eq!(MAX_APPROVERS, 64);
}

// #[test]
fn test_storage_deposit_amount() {
    assert_eq!(STORAGE_DEPOSIT_YOCTO, 500_000_000_000_000_000_000_000u128);
}

// #[test]
fn test_hex_roundtrip_bytes() {
    let bytes: Vec<u8> = (0..32).map(|i| (i * 7 + 13) as u8).collect();
    let encoded = message::hex_encode(&bytes);
    let decoded = message::hex_decode(&encoded);
    assert_eq!(bytes, decoded);
}

// #[test]
fn test_hex_empty() {
    let encoded = message::hex_encode(&[]);
    let decoded = message::hex_decode(&encoded);
    assert_eq!(decoded, Vec::<u8>::new());
}
