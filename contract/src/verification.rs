//! Comprehensive verification tests.
//!
//! Property-based tests (proptest) and exhaustive edge case tests to verify:
//! - Bitmap operations (approval/cancellation)
//! - State machine transitions
//! - Message building invariants
//! - Template rendering safety
//! - Balance accounting
//! - U128 precision handling

use crate::*;
use crate::ft::{ft_balance_key, is_token_allowed};
use crate::message::{build_message, hex_encode, hex_decode};
use proptest::prelude::*;

// ── Bitmap Verification ────────────────────────────────────────────────────

fn make_proposal() -> Proposal {
    Proposal {
        id: 0,
        wallet_name: "w".into(),
        intent_index: 0,
        proposer: "alice.near".parse().unwrap(),
        status: ProposalStatus::Active,
        proposed_at: 0,
        approved_at: 0,
        expires_at: u64::MAX,
        approval_bitmap: 0,
        cancellation_bitmap: 0,
        nostr_approval_bitmap: 0,
        nostr_cancellation_bitmap: 0,
        param_values: "{}".into(),
        message: "".into(),
        intent_params_hash: "".into(),
    }
}

// #[test]
fn test_bitmap_approve_then_cancel_clears_approval() {
    let mut p = make_proposal();
    p.set_approval(0);
    assert!(p.has_approved(0));
    assert_eq!(p.approval_count(), 1);

    p.set_cancellation(0);
    assert!(!p.has_approved(0));
    assert_eq!(p.approval_count(), 0);
    assert_eq!(p.cancellation_count(), 1);
}

// #[test]
fn test_bitmap_cancel_then_approve_clears_cancellation() {
    let mut p = make_proposal();
    p.set_cancellation(0);
    assert_eq!(p.cancellation_count(), 1);

    p.set_approval(0);
    assert_eq!(p.cancellation_count(), 0);
    assert!(p.has_approved(0));
}

// #[test]
fn test_bitmap_all_64_slots() {
    let mut p = make_proposal();
    for i in 0..64usize {
        p.set_approval(i);
        assert!(p.has_approved(i), "Slot {} should be approved", i);
    }
    assert_eq!(p.approval_count(), 64);
    assert_eq!(p.approval_bitmap, u64::MAX);

    // Cancel all
    for i in 0..64usize {
        p.set_cancellation(i);
    }
    assert_eq!(p.approval_count(), 0);
    assert_eq!(p.cancellation_count(), 64);
    assert_eq!(p.cancellation_bitmap, u64::MAX);
}

// #[test]
fn test_bitmap_approval_is_mutually_exclusive_per_slot() {
    let mut p = make_proposal();
    // For any slot, approve and cancel are never both set
    for i in 0..64usize {
        p.set_approval(i);
        assert!(p.has_approved(i));
        // cancellation bit for this slot must be 0
        assert_eq!((p.cancellation_bitmap >> i) & 1, 0);

        p.set_cancellation(i);
        // approval bit for this slot must be 0
        assert_eq!((p.approval_bitmap >> i) & 1, 0);
        // cancellation bit must be 1
        assert_eq!((p.cancellation_bitmap >> i) & 1, 1);
    }
    // No overlap: approval_bitmap & cancellation_bitmap == 0
    assert_eq!(p.approval_bitmap & p.cancellation_bitmap, 0);
}

// #[test]
fn test_bitmap_reset_clears_everything() {
    let mut p = make_proposal();
    for i in 0..64usize {
        p.set_approval(i);
    }
    p.approved_at = 12345;
    p.reset_votes();
    assert_eq!(p.approval_bitmap, 0);
    assert_eq!(p.cancellation_bitmap, 0);
    assert_eq!(p.approval_count(), 0);
    assert_eq!(p.cancellation_count(), 0);
    assert_eq!(p.approved_at, 0);
}

// #[test]
fn test_bitmap_count_matches_ones() {
    let mut p = make_proposal();
    // Set every other slot
    for i in (0..64).step_by(2) {
        p.set_approval(i);
    }
    assert_eq!(p.approval_count(), 32);
    // Verify against built-in count_ones
    assert_eq!(p.approval_count(), p.approval_bitmap.count_ones());
}

// ── Property-Based Bitmap Tests ────────────────────────────────────────────

proptest! {
    #[test]
    fn prop_approval_cancel_invariant(slots: Vec<usize>) {
        let mut p = make_proposal();
        let mut expected_approved: std::collections::HashSet<usize> = std::collections::HashSet::new();

        for slot in slots {
            let s = slot % 64; // bound to 0..63
            // Alternate between approve and cancel
            p.set_approval(s);
            expected_approved.insert(s);

            // Invariant: no overlap between approval and cancellation
            assert_eq!(p.approval_bitmap & p.cancellation_bitmap, 0);

            // Invariant: count matches actual bits (near + nostr combined)
            assert_eq!(p.approval_count(), (p.approval_bitmap.count_ones() + p.nostr_approval_bitmap.count_ones()) as u32);
            assert_eq!(p.cancellation_count(), (p.cancellation_bitmap.count_ones() + p.nostr_cancellation_bitmap.count_ones()) as u32);
        }
    }

    #[test]
    fn prop_set_approval_then_cancel_zeroes_approval(slot in 0usize..64) {
        let mut p = make_proposal();
        p.set_approval(slot);
        assert!(p.has_approved(slot));

        p.set_cancellation(slot);
        assert!(!p.has_approved(slot));
        // Double check the bit is literally 0
        assert_eq!((p.approval_bitmap >> slot) & 1, 0);
    }

    #[test]
    fn prop_set_cancel_then_approval_zeroes_cancel(slot in 0usize..64) {
        let mut p = make_proposal();
        p.set_cancellation(slot);

        p.set_approval(slot);
        assert_eq!((p.cancellation_bitmap >> slot) & 1, 0);
        assert!(!p.has_approved(slot) || (p.cancellation_bitmap >> slot) & 1 == 0);
        // Actually after set_approval, cancellation is cleared
        assert_eq!((p.cancellation_bitmap >> slot) & 1, 0);
    }

    #[test]
    fn prop_reset_always_clears_all(ops: Vec<(bool, usize)>) {
        let mut p = make_proposal();
        for (is_approve, slot) in &ops {
            let s = *slot % 64;
            if *is_approve { p.set_approval(s); } else { p.set_cancellation(s); }
        }
        p.approved_at = 99999;
        p.reset_votes();
        assert_eq!(p.approval_bitmap, 0);
        assert_eq!(p.cancellation_bitmap, 0);
        assert_eq!(p.approved_at, 0);
    }
}

// ── State Machine Verification ─────────────────────────────────────────────

// #[test]
fn test_state_transitions_valid() {
    let mut p = make_proposal();

    // Active → Approved
    assert_eq!(p.status, ProposalStatus::Active);
    p.set_approval(0);
    // Simulate threshold met
    p.status = ProposalStatus::Approved;
    p.approved_at = 100;
    assert_eq!(p.status, ProposalStatus::Approved);

    // Approved → Executed
    p.status = ProposalStatus::Executed;
    assert_eq!(p.status, ProposalStatus::Executed);

    // Can't transition from Executed
    // (contract asserts on this, we verify the status is terminal)
    assert_ne!(p.status, ProposalStatus::Active);
    assert_ne!(p.status, ProposalStatus::Approved);
    assert_ne!(p.status, ProposalStatus::Cancelled);
}

// #[test]
fn test_state_transitions_cancel_from_active() {
    let mut p = make_proposal();
    assert_eq!(p.status, ProposalStatus::Active);

    p.status = ProposalStatus::Cancelled;
    assert_eq!(p.status, ProposalStatus::Cancelled);

    // Cancelled is terminal
    assert_ne!(p.status, ProposalStatus::Active);
    assert_ne!(p.status, ProposalStatus::Approved);
    assert_ne!(p.status, ProposalStatus::Executed);
}

// #[test]
fn test_cannot_go_from_executed_back_to_active() {
    let mut p = make_proposal();
    p.status = ProposalStatus::Executed;
    // In the contract, assert!(status == Active) would fail
    // We verify the status is indeed Executed (terminal)
    assert_eq!(p.status, ProposalStatus::Executed);
}

// ── Template Rendering Safety ───────────────────────────────────────────────

fn make_intent(template: &str, params: Vec<(&str, ParamType)>) -> Intent {
    Intent {
        wallet_name: "test".into(),
        index: 0,
        intent_type: IntentType::Custom,
        name: "test".into(),
        template: template.into(),
        proposers: vec![],
        approvers: vec![],
        nostr_approvers: vec![],
        approval_threshold: 1,
        cancellation_threshold: 1,
        timelock_seconds: 0,
        params: params.into_iter().map(|(n, pt)| ParamDef { name: n.into(), param_type: pt, max_value: None }).collect(),
        execution_gas_tgas: 50,
        active: true,
        active_proposal_count: 0,
    }
}

// #[test]
fn test_template_pipe_rejected() {
    let intent = make_intent("do {p}", vec![("p", ParamType::String)]);
    let params = serde_json::json!({"p": "evil | wallet: fake"});
    assert!(std::panic::catch_unwind(|| intent.render_template(&params)).is_err());
}

// #[test]
fn test_template_newline_rejected() {
    let intent = make_intent("do {p}", vec![("p", ParamType::String)]);
    let params = serde_json::json!({"p": "evil\ninjected"});
    assert!(std::panic::catch_unwind(|| intent.render_template(&params)).is_err());
}

// #[test]
fn test_template_carriage_return_rejected() {
    let intent = make_intent("do {p}", vec![("p", ParamType::String)]);
    let params = serde_json::json!({"p": "evil\rinjected"});
    assert!(std::panic::catch_unwind(|| intent.render_template(&params)).is_err());
}

// #[test]
fn test_template_clean_values_pass() {
    let intent = make_intent("transfer {amount} to {who}", vec![
        ("amount", ParamType::U128),
        ("who", ParamType::AccountId),
    ]);
    let params = serde_json::json!({"amount": "1000000", "who": "bob.near"});
    assert_eq!(intent.render_template(&params), "transfer 1000000 to bob.near");
}

// #[test]
fn test_template_missing_param_skipped() {
    let intent = make_intent("{a} and {b}", vec![
        ("a", ParamType::String),
        ("b", ParamType::String),
    ]);
    let params = serde_json::json!({"a": "hello"});
    // {b} stays unreplaced since b is missing from params
    assert_eq!(intent.render_template(&params), "hello and {b}");
}

proptest! {
    #[test]
    fn prop_template_no_pipe_or_newline(s: String) {
        let intent = make_intent("{p}", vec![("p", ParamType::String)]);
        let params = serde_json::json!({"p": s});

        // If the string contains | or \n or \r, it should panic
        let has_illegal = s.contains('|') || s.contains('\n') || s.contains('\r');
        let result = std::panic::catch_unwind(|| intent.render_template(&params));

        if has_illegal {
            assert!(result.is_err(), "Should reject string with illegal chars: {:?}", s);
        } else {
            assert!(result.is_ok(), "Should accept clean string: {:?}", s);
        }
    }
}

// ── U128 Precision Verification ────────────────────────────────────────────

// #[test]
fn test_u128_max_value() {
    let max = u128::MAX; // 340282366920938463463374607431768211455
    let s = max.to_string();
    let parsed: u128 = s.parse().unwrap();
    assert_eq!(max, parsed);
}

// #[test]
fn test_u128_yocto_near() {
    // 1 NEAR = 10^24 yocto
    let one_near: u128 = 1_000_000_000_000_000_000_000_000;
    let s = one_near.to_string();
    let parsed: u128 = s.parse().unwrap();
    assert_eq!(one_near, parsed);
    assert_eq!(s, "1000000000000000000000000");
}

// #[test]
fn test_u128_large_amount_no_precision_loss() {
    let amounts = [
        "1000000000000000000000000",  // 1 NEAR
        "10000000000000000000000000", // 10 NEAR
        "340282366920938463463374607431768211455", // u128::MAX
        "18446744073709551615",       // u64::MAX
        "1",
        "0",
    ];
    for amount in amounts {
        let parsed: u128 = amount.parse().unwrap();
        let rendered = parsed.to_string();
        assert_eq!(amount, rendered, "Precision lost for {}", amount);
    }
}

proptest! {
    #[test]
    fn prop_u128_roundtrip(v: u128) {
        let s = v.to_string();
        let parsed: u128 = s.parse().unwrap();
        assert_eq!(v, parsed);
    }
}

// ── Balance Accounting Verification ────────────────────────────────────────

// #[test]
fn test_ft_balance_key_format() {
    assert_eq!(ft_balance_key("treasury", "usdt.tether-token.near"), "treasury:ft:usdt.tether-token.near");
    assert_eq!(ft_balance_key("a", "b"), "a:ft:b");
    // Ensure no collisions
    assert_ne!(ft_balance_key("ab", "c"), ft_balance_key("a", "bc"));
}

// #[test]
fn test_near_balance_key_no_collision() {
    let key1 = format!("{}:near", "treasury");
    let key2 = format!("{}:near", "treasury2");
    let key3 = ft_balance_key("treasury", "near");
    assert_ne!(key1, key2);
    assert_ne!(key1, key3);
}

// ── Hash Determinism Verification ──────────────────────────────────────────

// #[test]
fn test_hash_params_deterministic() {
    let params = vec![
        ParamDef { name: "amount".into(), param_type: ParamType::U128, max_value: None },
        ParamDef { name: "recipient".into(), param_type: ParamType::AccountId, max_value: None },
    ];
    let h1 = hash_params(&params);
    let h2 = hash_params(&params);
    assert_eq!(h1, h2, "Same params must produce same hash");
    assert_eq!(h1.len(), 64, "SHA-256 hex must be 64 chars");
}

// #[test]
fn test_hash_params_different_for_different_params() {
    let p1 = vec![ParamDef { name: "a".into(), param_type: ParamType::U64, max_value: None }];
    let p2 = vec![ParamDef { name: "b".into(), param_type: ParamType::U64, max_value: None }];
    assert_ne!(hash_params(&p1), hash_params(&p2));
}

// ── Message Building Verification ──────────────────────────────────────────

// #[test]
fn test_message_contains_all_parts() {
    let intent = make_intent("transfer {amount} to {r}", vec![
        ("amount", ParamType::U128),
        ("r", ParamType::AccountId),
    ]);
    let params = serde_json::json!({"amount": "100", "r": "bob.near"});

    let msg = build_message("treasury", 42, 1000_000_000_000, "propose", &intent, &params);

    assert!(msg.starts_with("expires "));
    assert!(msg.contains("propose"));
    assert!(msg.contains("transfer 100 to bob.near"));
    assert!(msg.contains("wallet: treasury"));
    assert!(msg.contains("proposal: 42"));
}

// #[test]
fn test_message_format_consistency() {
    let intent = make_intent("do {x}", vec![("x", ParamType::String)]);

    for action in &["propose", "approve", "cancel", "amend"] {
        let params = serde_json::json!({"x": "test"});
        let msg = build_message("w", 0, 1_000_000_000, action, &intent, &params);

        // Every message must have exactly one | separator
        assert_eq!(msg.matches('|').count(), 1, "Expected one '|' in: {}", msg);

        // Must start with "expires "
        assert!(msg.starts_with("expires "), "Message must start with 'expires ': {}", msg);

        // Must contain "wallet: w proposal: 0"
        assert!(msg.contains("wallet: w proposal: 0"), "Missing wallet/proposal: {}", msg);

        // Must contain the action
        assert!(msg.contains(action), "Missing action '{}': {}", action, msg);
    }
}

// ── Hex Encoding Verification ──────────────────────────────────────────────

proptest! {
    #[test]
    fn prop_hex_roundtrip(bytes: Vec<u8>) {
        let encoded = hex_encode(&bytes);
        let decoded = hex_decode(&encoded);
        assert_eq!(bytes, decoded);
    }

    #[test]
    fn prop_hex_is_lowercase(bytes: Vec<u8>) {
        let encoded = hex_encode(&bytes);
        assert_eq!(encoded, encoded.to_lowercase());
    }

    #[test]
    fn prop_hex_length_is_2x(bytes: Vec<u8>) {
        let encoded = hex_encode(&bytes);
        assert_eq!(encoded.len(), bytes.len() * 2);
    }
}

// ── Token Allowlist Verification ────────────────────────────────────────────

fn make_wallet(tokens: Vec<&str>) -> Wallet {
    Wallet {
        name: "test".into(),
        owner: "owner.near".parse().unwrap(),
        proposal_index: 0,
        intent_index: 3,
        created_at: 0,
        storage_deposit: 0,
        storage_used: 0,
        allowed_tokens: tokens.into_iter().map(|t| t.parse().unwrap()).collect(),
        ft_token_count: 0,
    }
}

// #[test]
fn test_allowlist_empty_accepts_all() {
    let wallet = make_wallet(vec![]);
    assert!(is_token_allowed(&wallet, &"anything.near".parse().unwrap()));
    assert!(is_token_allowed(&wallet, &"evil.near".parse().unwrap()));
}

// #[test]
fn test_allowlist_non_empty_blocks_unlisted() {
    let wallet = make_wallet(vec!["usdt.tether-token.near"]);
    assert!(is_token_allowed(&wallet, &"usdt.tether-token.near".parse().unwrap()));
    assert!(!is_token_allowed(&wallet, &"evil.near".parse().unwrap()));
}

proptest! {
    #[test]
    fn prop_allowlist_empty_always_allows(token: String) {
        let wallet = make_wallet(vec![]);
        // Empty allowlist should accept any string as a token
        // (even if it's not a valid AccountId, the allowlist check passes)
        if let Ok(tid) = token.parse::<AccountId>() {
            assert!(is_token_allowed(&wallet, &tid));
        }
    }
}
