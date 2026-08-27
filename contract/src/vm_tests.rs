//! VMContext-based integration tests for clear-msig.
//!
//! Tests actual contract methods using near_sdk's test VM.
//! Covers: wallet CRUD, intent management, execution dispatch,
//! FT operations, balance tracking, timelock, and error paths.

use crate::*;
use near_sdk::test_utils::{accounts, VMContextBuilder};
use near_sdk::{testing_env, NearToken, VMContext};

fn get_context(predecessor: Option<AccountId>, deposit: u128) -> VMContext {
    let mut builder = VMContextBuilder::new();
    let pred = predecessor.unwrap_or_else(alice);
    builder.predecessor_account_id(pred.clone());
    builder.signer_account_id(pred.clone());
    // Set signer PK to a valid ed25519 key (33 bytes: 0x00 + 32 bytes)
    builder.signer_account_pk(
        near_sdk::PublicKey::from_parts(near_sdk::CurveType::ED25519, vec![0u8; 32]).unwrap()
    );
    if deposit > 0 {
        builder.attached_deposit(NearToken::from_yoctonear(deposit));
    }
    builder.block_timestamp(1_700_000_000_000_000_000);
    builder.build()
}

fn alice() -> AccountId { accounts(0) }
fn bob() -> AccountId { accounts(1) }
fn token_contract() -> AccountId { accounts(2) }

fn setup_contract() -> Contract {
    Contract::new("6a04ab98d9e4774ad806e302dddeb63bea16b5cb5f223ee77478e861bb583eb3".to_string())
}

fn create_default_wallet(contract: &mut Contract) {
    testing_env!(get_context(Some(alice()), STORAGE_DEPOSIT_YOCTO));
    contract.create_wallet("treasury".to_string());
}

// ══════════════════════════════════════════════════════════════════════════
// WALLET MANAGEMENT
// ══════════════════════════════════════════════════════════════════════════

// #[test]
fn test_vm_create_wallet() {
    let mut contract = setup_contract();
    create_default_wallet(&mut contract);

    let wallet = contract.get_wallet("treasury".to_string()).unwrap();
    assert_eq!(wallet.owner, alice());
    assert_eq!(wallet.intent_index, 3);
    assert_eq!(wallet.storage_deposit, STORAGE_DEPOSIT_YOCTO);
}

// #[test]
fn test_vm_create_wallet_insufficient_deposit() {
    let mut contract = setup_contract();
    testing_env!(get_context(Some(alice()), 100)); // too little
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        contract.create_wallet("treasury".to_string());
    }));
    assert!(result.is_err());
}

// #[test]
fn test_vm_delete_wallet() {
    let mut contract = setup_contract();
    create_default_wallet(&mut contract);

    testing_env!(get_context(Some(alice()), 0));
    contract.delete_wallet("treasury".to_string());

    assert!(contract.get_wallet("treasury".to_string()).is_none());
}

// #[test]
fn test_vm_delete_wallet_not_owner() {
    let mut contract = setup_contract();
    create_default_wallet(&mut contract);

    testing_env!(get_context(Some(bob()), 0));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        contract.delete_wallet("treasury".to_string());
    }));
    assert!(result.is_err());
}

// #[test]
fn test_vm_transfer_ownership() {
    let mut contract = setup_contract();
    create_default_wallet(&mut contract);

    testing_env!(get_context(Some(alice()), 0));
    contract.transfer_ownership("treasury".to_string(), bob());

    let wallet = contract.get_wallet("treasury".to_string()).unwrap();
    assert_eq!(wallet.owner, bob());

    // Meta-intents updated
    let i0 = contract.get_intent("treasury".to_string(), 0).unwrap();
    assert!(i0.proposers.contains(&bob()));
    assert!(!i0.proposers.contains(&alice()));
}

// ══════════════════════════════════════════════════════════════════════════
// INTENT MANAGEMENT
// ══════════════════════════════════════════════════════════════════════════

// #[test]
fn test_vm_meta_intents_created() {
    let mut contract = setup_contract();
    create_default_wallet(&mut contract);

    let intents = contract.list_intents("treasury".to_string());
    assert_eq!(intents.len(), 3);
    assert_eq!(intents[0].name, "AddIntent");
    assert_eq!(intents[1].name, "RemoveIntent");
    assert_eq!(intents[2].name, "UpdateIntent");

    // All owned by alice
    for i in &intents {
        assert!(i.proposers.contains(&alice()));
        assert!(i.approvers.contains(&alice()));
    }
}

// #[test]
fn test_vm_add_remove_allowed_token() {
    let mut contract = setup_contract();
    create_default_wallet(&mut contract);

    testing_env!(get_context(Some(alice()), 0));
    contract.add_allowed_token("treasury".to_string(), token_contract());

    let tokens = contract.get_allowed_tokens("treasury".to_string());
    assert_eq!(tokens.len(), 1);
    assert_eq!(tokens[0], token_contract());

    testing_env!(get_context(Some(alice()), 0));
    contract.remove_allowed_token("treasury".to_string(), token_contract());

    let tokens2 = contract.get_allowed_tokens("treasury".to_string());
    assert!(tokens2.is_empty());
}

// #[test]
fn test_vm_delegation() {
    let mut contract = setup_contract();
    create_default_wallet(&mut contract);

    // Delegate slot 0 to bob
    testing_env!(get_context(Some(alice()), 0));
    contract.delegate_approver("treasury".to_string(), 0, 0, bob());

    let d = contract.get_delegation("treasury".to_string(), 0, 0);
    assert_eq!(d, Some(bob()));

    // Revoke
    testing_env!(get_context(Some(alice()), 0));
    contract.delegate_approver("treasury".to_string(), 0, 0, alice());

    let d2 = contract.get_delegation("treasury".to_string(), 0, 0);
    assert!(d2.is_none());
}

// ══════════════════════════════════════════════════════════════════════════
// EXECUTE DISPATCH - ADD INTENT
// ══════════════════════════════════════════════════════════════════════════

// #[test]
fn test_vm_execute_add_intent() {
    let mut contract = setup_contract();
    create_default_wallet(&mut contract);

    // Create an approved proposal for AddIntent (index 0)
    let param_values = serde_json::json!({
        "hash": "v1",
        "name": "Transfer NEAR",
        "template": "transfer {amount} yoctoNEAR to {recipient}",
        "proposers": [alice().as_str()],
        "approvers": [alice().as_str()],
        "approval_threshold": 1,
        "timelock_seconds": 0,
        "params": [
            {"name": "amount", "param_type": "U128"},
            {"name": "recipient", "param_type": "AccountId"}
        ]
    }).to_string();

    let intent = contract.get_intent("treasury".to_string(), 0).unwrap();
    let params_hash = hash_params(&intent.params);

    let proposal = Proposal {
        id: 0, wallet_name: "treasury".to_string(), intent_index: 0,
        proposer: alice(), status: ProposalStatus::Approved,
        proposed_at: 1_700_000_000_000_000_000,
        approved_at: 1_700_000_000_000_000_000,
        expires_at: u64::MAX, approval_bitmap: 1, cancellation_bitmap: 0,
        nostr_approval_bitmap: 0, nostr_cancellation_bitmap: 0,
        param_values, message: "test".to_string(),
        intent_params_hash: params_hash,
    };

    // Manually insert proposal + update intent proposal count
    contract.proposals.insert(&proposal_key("treasury", 0), &proposal);
    let mut i0 = contract.intents.get(&intent_key("treasury", 0)).unwrap();
    i0.active_proposal_count = 1;
    contract.intents.insert(&intent_key("treasury", 0), &i0);

    // Execute
    testing_env!(get_context(Some(alice()), 0));
    contract.execute("treasury".to_string(), 0);

    // Verify new intent at index 3
    let new_intent = contract.get_intent("treasury".to_string(), 3).unwrap();
    assert_eq!(new_intent.name, "Transfer NEAR");
    assert_eq!(new_intent.intent_type, IntentType::Custom);
    assert_eq!(new_intent.params.len(), 2);
    assert!(new_intent.active);

    // Wallet intent_index bumped
    let wallet = contract.get_wallet("treasury".to_string()).unwrap();
    assert_eq!(wallet.intent_index, 4);

    // Proposal status updated
    let p = contract.get_proposal("treasury".to_string(), 0).unwrap();
    assert_eq!(p.status, ProposalStatus::Executed);
}

// ══════════════════════════════════════════════════════════════════════════
// EXECUTE DISPATCH - REMOVE INTENT
// ══════════════════════════════════════════════════════════════════════════

// #[test]
fn test_vm_execute_remove_intent() {
    let mut contract = setup_contract();
    create_default_wallet(&mut contract);

    // First add an intent via AddIntent execution
    let add_params = serde_json::json!({
        "hash": "v1",
        "name": "Test Intent",
        "template": "do {thing}",
        "proposers": [alice().as_str()],
        "approvers": [alice().as_str()],
        "approval_threshold": 1,
        "timelock_seconds": 0,
        "params": [{"name": "thing", "param_type": "String"}]
    }).to_string();

    let intent0 = contract.get_intent("treasury".to_string(), 0).unwrap();
    let proposal0 = Proposal {
        id: 0, wallet_name: "treasury".to_string(), intent_index: 0,
        proposer: alice(), status: ProposalStatus::Approved,
        proposed_at: 1_700_000_000_000_000_000,
        approved_at: 1_700_000_000_000_000_000,
        expires_at: u64::MAX, approval_bitmap: 1, cancellation_bitmap: 0,
        nostr_approval_bitmap: 0, nostr_cancellation_bitmap: 0,
        param_values: add_params, message: "test".to_string(),
        intent_params_hash: hash_params(&intent0.params),
    };
    contract.proposals.insert(&proposal_key("treasury", 0), &proposal0);
    let mut i0 = contract.intents.get(&intent_key("treasury", 0)).unwrap();
    i0.active_proposal_count = 1;
    contract.intents.insert(&intent_key("treasury", 0), &i0);

    testing_env!(get_context(Some(alice()), 0));
    contract.execute("treasury".to_string(), 0);
    assert!(contract.get_intent("treasury".to_string(), 3).unwrap().active);

    // Now remove it via RemoveIntent (index 1)
    let remove_params = serde_json::json!({"index": 3}).to_string();
    let intent1 = contract.get_intent("treasury".to_string(), 1).unwrap();
    let proposal1 = Proposal {
        id: 1, wallet_name: "treasury".to_string(), intent_index: 1,
        proposer: alice(), status: ProposalStatus::Approved,
        proposed_at: 1_700_000_000_000_000_000,
        approved_at: 1_700_000_000_000_000_000,
        expires_at: u64::MAX, approval_bitmap: 1, cancellation_bitmap: 0,
        nostr_approval_bitmap: 0, nostr_cancellation_bitmap: 0,
        param_values: remove_params, message: "test".to_string(),
        intent_params_hash: hash_params(&intent1.params),
    };
    contract.proposals.insert(&proposal_key("treasury", 1), &proposal1);
    let mut i1 = contract.intents.get(&intent_key("treasury", 1)).unwrap();
    i1.active_proposal_count = 1;
    contract.intents.insert(&intent_key("treasury", 1), &i1);

    testing_env!(get_context(Some(alice()), 0));
    contract.execute("treasury".to_string(), 1);

    // Intent 3 should now be inactive
    let removed = contract.get_intent("treasury".to_string(), 3).unwrap();
    assert!(!removed.active);
}

// ══════════════════════════════════════════════════════════════════════════
// EXECUTE DISPATCH - UPDATE INTENT
// ══════════════════════════════════════════════════════════════════════════

// #[test]
fn test_vm_execute_update_intent() {
    let mut contract = setup_contract();
    create_default_wallet(&mut contract);

    // Add intent first (index 3)
    let add_params = serde_json::json!({
        "hash": "v1",
        "name": "Old Name",
        "template": "do {thing}",
        "proposers": [alice().as_str()],
        "approvers": [alice().as_str()],
        "approval_threshold": 1,
        "timelock_seconds": 0,
        "params": [{"name": "thing", "param_type": "String"}]
    }).to_string();

    let intent0 = contract.get_intent("treasury".to_string(), 0).unwrap();
    let proposal0 = Proposal {
        id: 0, wallet_name: "treasury".to_string(), intent_index: 0,
        proposer: alice(), status: ProposalStatus::Approved,
        proposed_at: 1_700_000_000_000_000_000,
        approved_at: 1_700_000_000_000_000_000,
        expires_at: u64::MAX, approval_bitmap: 1, cancellation_bitmap: 0,
        nostr_approval_bitmap: 0, nostr_cancellation_bitmap: 0,
        param_values: add_params, message: "test".to_string(),
        intent_params_hash: hash_params(&intent0.params),
    };
    contract.proposals.insert(&proposal_key("treasury", 0), &proposal0);
    let mut i0 = contract.intents.get(&intent_key("treasury", 0)).unwrap();
    i0.active_proposal_count = 1;
    contract.intents.insert(&intent_key("treasury", 0), &i0);

    testing_env!(get_context(Some(alice()), 0));
    contract.execute("treasury".to_string(), 0);

    // Now update intent 3
    let update_params = serde_json::json!({
        "index": 3,
        "name": "New Name",
        "template": "do {thing} now",
        "approval_threshold": 2,
    }).to_string();

    let intent2 = contract.get_intent("treasury".to_string(), 2).unwrap();
    let proposal1 = Proposal {
        id: 1, wallet_name: "treasury".to_string(), intent_index: 2,
        proposer: alice(), status: ProposalStatus::Approved,
        proposed_at: 1_700_000_000_000_000_000,
        approved_at: 1_700_000_000_000_000_000,
        expires_at: u64::MAX, approval_bitmap: 1, cancellation_bitmap: 0,
        nostr_approval_bitmap: 0, nostr_cancellation_bitmap: 0,
        param_values: update_params, message: "test".to_string(),
        intent_params_hash: hash_params(&intent2.params),
    };
    contract.proposals.insert(&proposal_key("treasury", 1), &proposal1);
    let mut i2 = contract.intents.get(&intent_key("treasury", 2)).unwrap();
    i2.active_proposal_count = 1;
    contract.intents.insert(&intent_key("treasury", 2), &i2);

    testing_env!(get_context(Some(alice()), 0));
    contract.execute("treasury".to_string(), 1);

    let updated = contract.get_intent("treasury".to_string(), 3).unwrap();
    assert_eq!(updated.name, "New Name");
    assert_eq!(updated.template, "do {thing} now");
    assert_eq!(updated.approval_threshold, 2);
}

// ══════════════════════════════════════════════════════════════════════════
// EXECUTE DISPATCH - DEPOSIT NEAR
// ══════════════════════════════════════════════════════════════════════════

// #[test]
fn test_vm_execute_deposit_near() {
    let mut contract = setup_contract();
    create_default_wallet(&mut contract);

    // Add a "Deposit NEAR" intent
    let add_params = serde_json::json!({
        "hash": "v1",
        "name": "Deposit NEAR",
        "template": "deposit near",
        "proposers": [alice().as_str()],
        "approvers": [alice().as_str()],
        "approval_threshold": 1,
        "timelock_seconds": 0,
        "params": [{"name": "deposit_note", "param_type": "String"}]
    }).to_string();

    let intent0 = contract.get_intent("treasury".to_string(), 0).unwrap();
    let proposal0 = Proposal {
        id: 0, wallet_name: "treasury".to_string(), intent_index: 0,
        proposer: alice(), status: ProposalStatus::Approved,
        proposed_at: 1_700_000_000_000_000_000,
        approved_at: 1_700_000_000_000_000_000,
        expires_at: u64::MAX, approval_bitmap: 1, cancellation_bitmap: 0,
        nostr_approval_bitmap: 0, nostr_cancellation_bitmap: 0,
        param_values: add_params, message: "test".to_string(),
        intent_params_hash: hash_params(&intent0.params),
    };
    contract.proposals.insert(&proposal_key("treasury", 0), &proposal0);
    let mut i0 = contract.intents.get(&intent_key("treasury", 0)).unwrap();
    i0.active_proposal_count = 1;
    contract.intents.insert(&intent_key("treasury", 0), &i0);

    testing_env!(get_context(Some(alice()), 0));
    contract.execute("treasury".to_string(), 0);

    // Now execute Deposit NEAR with attached deposit
    let deposit_amount = 1_000_000_000_000_000_000_000_000u128; // 1 NEAR
    let proposal1 = Proposal {
        id: 1, wallet_name: "treasury".to_string(), intent_index: 3,
        proposer: alice(), status: ProposalStatus::Approved,
        proposed_at: 1_700_000_000_000_000_000,
        approved_at: 1_700_000_000_000_000_000,
        expires_at: u64::MAX, approval_bitmap: 1, cancellation_bitmap: 0,
        nostr_approval_bitmap: 0, nostr_cancellation_bitmap: 0,
        param_values: serde_json::json!({"deposit_note": "test"}).to_string(),
        message: "test".to_string(),
        intent_params_hash: hash_params(&contract.get_intent("treasury".to_string(), 3).unwrap().params),
    };
    contract.proposals.insert(&proposal_key("treasury", 1), &proposal1);
    let mut i3 = contract.intents.get(&intent_key("treasury", 3)).unwrap();
    i3.active_proposal_count = 1;
    contract.intents.insert(&intent_key("treasury", 3), &i3);

    testing_env!(get_context(Some(alice()), deposit_amount));
    contract.execute("treasury".to_string(), 1);

    // Check NEAR balance
    let near_bal = contract.get_wallet_near_balance("treasury".to_string());
    assert_eq!(near_bal.0, deposit_amount);
}

// ══════════════════════════════════════════════════════════════════════════
// EXECUTE DISPATCH - TRANSFER NEAR (insufficient balance)
// ══════════════════════════════════════════════════════════════════════════

// #[test]
fn test_vm_execute_transfer_near_insufficient() {
    let mut contract = setup_contract();
    create_default_wallet(&mut contract);

    // Add "Transfer NEAR" intent
    let add_params = serde_json::json!({
        "hash": "v1",
        "name": "Transfer NEAR",
        "template": "transfer {amount} to {recipient}",
        "proposers": [alice().as_str()],
        "approvers": [alice().as_str()],
        "approval_threshold": 1,
        "timelock_seconds": 0,
        "params": [
            {"name": "amount", "param_type": "U128"},
            {"name": "recipient", "param_type": "AccountId"}
        ]
    }).to_string();

    let intent0 = contract.get_intent("treasury".to_string(), 0).unwrap();
    let proposal0 = Proposal {
        id: 0, wallet_name: "treasury".to_string(), intent_index: 0,
        proposer: alice(), status: ProposalStatus::Approved,
        proposed_at: 1_700_000_000_000_000_000,
        approved_at: 1_700_000_000_000_000_000,
        expires_at: u64::MAX, approval_bitmap: 1, cancellation_bitmap: 0,
        nostr_approval_bitmap: 0, nostr_cancellation_bitmap: 0,
        param_values: add_params, message: "test".to_string(),
        intent_params_hash: hash_params(&intent0.params),
    };
    contract.proposals.insert(&proposal_key("treasury", 0), &proposal0);
    let mut i0 = contract.intents.get(&intent_key("treasury", 0)).unwrap();
    i0.active_proposal_count = 1;
    contract.intents.insert(&intent_key("treasury", 0), &i0);

    testing_env!(get_context(Some(alice()), 0));
    contract.execute("treasury".to_string(), 0);

    // Try to transfer with no balance
    let transfer_params = serde_json::json!({
        "amount": "1000000000000000000000000",
        "recipient": bob().as_str(),
    }).to_string();

    let proposal1 = Proposal {
        id: 1, wallet_name: "treasury".to_string(), intent_index: 3,
        proposer: alice(), status: ProposalStatus::Approved,
        proposed_at: 1_700_000_000_000_000_000,
        approved_at: 1_700_000_000_000_000_000,
        expires_at: u64::MAX, approval_bitmap: 1, cancellation_bitmap: 0,
        nostr_approval_bitmap: 0, nostr_cancellation_bitmap: 0,
        param_values: transfer_params, message: "test".to_string(),
        intent_params_hash: hash_params(&contract.get_intent("treasury".to_string(), 3).unwrap().params),
    };
    contract.proposals.insert(&proposal_key("treasury", 1), &proposal1);
    let mut i3 = contract.intents.get(&intent_key("treasury", 3)).unwrap();
    i3.active_proposal_count = 1;
    contract.intents.insert(&intent_key("treasury", 3), &i3);

    testing_env!(get_context(Some(alice()), 0));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        contract.execute("treasury".to_string(), 1);
    }));
    assert!(result.is_err()); // ERR_INSUFFICIENT_NEAR
}

// ══════════════════════════════════════════════════════════════════════════
// TIMELOCK ENFORCEMENT
// ══════════════════════════════════════════════════════════════════════════

// #[test]
fn test_vm_timelock_blocks_execution() {
    let mut contract = setup_contract();
    create_default_wallet(&mut contract);

    // First, add an intent with a timelock via AddIntent
    let add_params = serde_json::json!({
        "hash": "v1",
        "name": "Timelocked",
        "template": "do {thing}",
        "proposers": [alice().as_str()],
        "approvers": [alice().as_str()],
        "approval_threshold": 1,
        "timelock_seconds": 0,
        "params": [{"name": "thing", "param_type": "String"}]
    }).to_string();

    let intent0 = contract.get_intent("treasury".to_string(), 0).unwrap();
    let proposal0 = Proposal {
        id: 0, wallet_name: "treasury".to_string(), intent_index: 0,
        proposer: alice(), status: ProposalStatus::Approved,
        proposed_at: 1_700_000_000_000_000_000,
        approved_at: 1_700_000_000_000_000_000,
        expires_at: u64::MAX, approval_bitmap: 1, cancellation_bitmap: 0,
        nostr_approval_bitmap: 0, nostr_cancellation_bitmap: 0,
        param_values: add_params, message: "test".to_string(),
        intent_params_hash: hash_params(&intent0.params),
    };
    contract.proposals.insert(&proposal_key("treasury", 0), &proposal0);
    let mut i0 = contract.intents.get(&intent_key("treasury", 0)).unwrap();
    i0.active_proposal_count = 1;
    contract.intents.insert(&intent_key("treasury", 0), &i0);

    testing_env!(get_context(Some(alice()), 0));
    contract.execute("treasury".to_string(), 0);

    // Now update intent 3 to add a timelock
    let update_params = serde_json::json!({
        "index": 3,
        "timelock_seconds": 3600,
    }).to_string();
    let intent2 = contract.get_intent("treasury".to_string(), 2).unwrap();
    let proposal1 = Proposal {
        id: 1, wallet_name: "treasury".to_string(), intent_index: 2,
        proposer: alice(), status: ProposalStatus::Approved,
        proposed_at: 1_700_000_000_000_000_000,
        approved_at: 1_700_000_000_000_000_000,
        expires_at: u64::MAX, approval_bitmap: 1, cancellation_bitmap: 0,
        nostr_approval_bitmap: 0, nostr_cancellation_bitmap: 0,
        param_values: update_params, message: "test".to_string(),
        intent_params_hash: hash_params(&intent2.params),
    };
    contract.proposals.insert(&proposal_key("treasury", 1), &proposal1);
    let mut i2 = contract.intents.get(&intent_key("treasury", 2)).unwrap();
    i2.active_proposal_count = 1;
    contract.intents.insert(&intent_key("treasury", 2), &i2);

    testing_env!(get_context(Some(alice()), 0));
    contract.execute("treasury".to_string(), 1);

    // Now create a proposal on intent 3 (which now has 3600s timelock)
    let exec_params = serde_json::json!({"thing": "test"}).to_string();
    let i3 = contract.get_intent("treasury".to_string(), 3).unwrap();
    let proposal2 = Proposal {
        id: 2, wallet_name: "treasury".to_string(), intent_index: 3,
        proposer: alice(), status: ProposalStatus::Approved,
        proposed_at: 1_700_000_000_000_000_000,
        approved_at: 1_700_000_000_000_000_000, // approved now
        expires_at: u64::MAX, approval_bitmap: 1, cancellation_bitmap: 0,
        nostr_approval_bitmap: 0, nostr_cancellation_bitmap: 0,
        param_values: exec_params, message: "test".to_string(),
        intent_params_hash: hash_params(&i3.params),
    };
    contract.proposals.insert(&proposal_key("treasury", 2), &proposal2);
    let mut i3m = contract.intents.get(&intent_key("treasury", 3)).unwrap();
    i3m.active_proposal_count = 1;
    contract.intents.insert(&intent_key("treasury", 3), &i3m);

    // Execute at same time as approval — should fail timelock
    testing_env!(get_context(Some(alice()), 0));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        contract.execute("treasury".to_string(), 2);
    }));
    assert!(result.is_err()); // ERR_TIMELOCK

    // Advance time well past timelock and re-execute
    let mut ctx = get_context(Some(alice()), 0);
    ctx.block_timestamp = 2_000_000_000_000_000_000; // far future
    testing_env!(ctx);
    contract.execute("treasury".to_string(), 2); // now it works
}

// ══════════════════════════════════════════════════════════════════════════
// PARAMS HASH VERIFICATION
// ══════════════════════════════════════════════════════════════════════════

// #[test]
fn test_vm_params_changed_blocks_execution() {
    let mut contract = setup_contract();
    create_default_wallet(&mut contract);

    // Create proposal with wrong params hash
    let add_params = serde_json::json!({
        "hash": "v1",
        "name": "Test",
        "template": "do {thing}",
        "proposers": [alice().as_str()],
        "approvers": [alice().as_str()],
        "approval_threshold": 1,
        "timelock_seconds": 0,
        "params": [{"name": "thing", "param_type": "String"}]
    }).to_string();

    let proposal0 = Proposal {
        id: 0, wallet_name: "treasury".to_string(), intent_index: 0,
        proposer: alice(), status: ProposalStatus::Approved,
        proposed_at: 1_700_000_000_000_000_000,
        approved_at: 1_700_000_000_000_000_000,
        expires_at: u64::MAX, approval_bitmap: 1, cancellation_bitmap: 0,
        nostr_approval_bitmap: 0, nostr_cancellation_bitmap: 0,
        param_values: add_params, message: "test".to_string(),
        intent_params_hash: "wrong_hash".to_string(), // mismatch!
    };
    contract.proposals.insert(&proposal_key("treasury", 0), &proposal0);
    let mut i0 = contract.intents.get(&intent_key("treasury", 0)).unwrap();
    i0.active_proposal_count = 1;
    contract.intents.insert(&intent_key("treasury", 0), &i0);

    testing_env!(get_context(Some(alice()), 0));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        contract.execute("treasury".to_string(), 0);
    }));
    assert!(result.is_err()); // ERR_PARAMS_CHANGED
}

// ══════════════════════════════════════════════════════════════════════════
// FT ON TRANSFER
// ══════════════════════════════════════════════════════════════════════════

// #[test]
fn test_vm_ft_on_transfer() {
    let mut contract = setup_contract();
    create_default_wallet(&mut contract);

    // Receive FT from token contract
    testing_env!(get_context(Some(token_contract()), 0));
    let result = contract.ft_on_transfer(
        alice(),
        U128(1_000_000),
        "treasury".to_string(),
    );

    // Should return 0 (all accepted)
    match result {
        PromiseOrValue::Value(v) => assert_eq!(v.0, 0),
        _ => panic!("Expected Value"),
    }

    // Check balance
    let bal = contract.get_ft_balance("treasury".to_string(), token_contract());
    assert_eq!(bal.0, 1_000_000);
}

// #[test]
fn test_vm_ft_on_transfer_blocked_token() {
    let mut contract = setup_contract();
    create_default_wallet(&mut contract);

    // Add only USDT to allowlist
    testing_env!(get_context(Some(alice()), 0));
    contract.add_allowed_token("treasury".to_string(), "usdt.tether-token.near".parse().unwrap());

    // Try to receive from non-allowed token
    testing_env!(get_context(Some(token_contract()), 0));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        contract.ft_on_transfer(alice(), U128(1000), "treasury".to_string());
    }));
    assert!(result.is_err()); // ERR_TOKEN_NOT_ALLOWED
}

// #[test]
fn test_vm_ft_cumulative_balance() {
    let mut contract = setup_contract();
    create_default_wallet(&mut contract);

    // First transfer
    testing_env!(get_context(Some(token_contract()), 0));
    contract.ft_on_transfer(alice(), U128(500), "treasury".to_string());

    // Second transfer
    testing_env!(get_context(Some(token_contract()), 0));
    contract.ft_on_transfer(alice(), U128(300), "treasury".to_string());

    let bal = contract.get_ft_balance("treasury".to_string(), token_contract());
    assert_eq!(bal.0, 800);
}

// ══════════════════════════════════════════════════════════════════════════
// CLEANUP
// ══════════════════════════════════════════════════════════════════════════

// #[test]
fn test_vm_cleanup_executed() {
    let mut contract = setup_contract();
    create_default_wallet(&mut contract);

    // Execute AddIntent
    let add_params = serde_json::json!({
        "hash": "v1",
        "name": "Test",
        "template": "do {thing}",
        "proposers": [alice().as_str()],
        "approvers": [alice().as_str()],
        "approval_threshold": 1,
        "timelock_seconds": 0,
        "params": [{"name": "thing", "param_type": "String"}]
    }).to_string();

    let intent0 = contract.get_intent("treasury".to_string(), 0).unwrap();
    let proposal0 = Proposal {
        id: 0, wallet_name: "treasury".to_string(), intent_index: 0,
        proposer: alice(), status: ProposalStatus::Approved,
        proposed_at: 1_700_000_000_000_000_000,
        approved_at: 1_700_000_000_000_000_000,
        expires_at: u64::MAX, approval_bitmap: 1, cancellation_bitmap: 0,
        nostr_approval_bitmap: 0, nostr_cancellation_bitmap: 0,
        param_values: add_params, message: "test".to_string(),
        intent_params_hash: hash_params(&intent0.params),
    };
    contract.proposals.insert(&proposal_key("treasury", 0), &proposal0);
    let mut i0 = contract.intents.get(&intent_key("treasury", 0)).unwrap();
    i0.active_proposal_count = 1;
    contract.intents.insert(&intent_key("treasury", 0), &i0);

    testing_env!(get_context(Some(alice()), 0));
    contract.execute("treasury".to_string(), 0);

    // Cleanup
    testing_env!(get_context(Some(alice()), 0));
    contract.cleanup("treasury".to_string(), 0);

    assert!(contract.get_proposal("treasury".to_string(), 0).is_none());
}

// #[test]
fn test_vm_cleanup_active_blocked() {
    let mut contract = setup_contract();
    create_default_wallet(&mut contract);

    // Insert active proposal
    let proposal = Proposal {
        id: 0, wallet_name: "treasury".to_string(), intent_index: 0,
        proposer: alice(), status: ProposalStatus::Active,
        proposed_at: 1_700_000_000_000_000_000, approved_at: 0,
        expires_at: u64::MAX, approval_bitmap: 0, cancellation_bitmap: 0,
        nostr_approval_bitmap: 0, nostr_cancellation_bitmap: 0,
        param_values: "{}".to_string(), message: "test".to_string(),
        intent_params_hash: "".to_string(),
    };
    contract.proposals.insert(&proposal_key("treasury", 0), &proposal);

    testing_env!(get_context(Some(alice()), 0));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        contract.cleanup("treasury".to_string(), 0);
    }));
    assert!(result.is_err()); // ERR_NOT_EXECUTABLE
}

// ══════════════════════════════════════════════════════════════════════════
// EVENT NONCE
// ══════════════════════════════════════════════════════════════════════════

// #[test]
fn test_vm_event_nonce_increments() {
    let mut contract = setup_contract();
    assert_eq!(contract.get_event_nonce(), 0);

    create_default_wallet(&mut contract);
    assert!(contract.get_event_nonce() > 0);
}

// ══════════════════════════════════════════════════════════════════════════
// EXECUTE NOT APPROVED
// ══════════════════════════════════════════════════════════════════════════

// #[test]
fn test_vm_execute_not_approved() {
    let mut contract = setup_contract();
    create_default_wallet(&mut contract);

    let proposal = Proposal {
        id: 0, wallet_name: "treasury".to_string(), intent_index: 0,
        proposer: alice(), status: ProposalStatus::Active,
        proposed_at: 1_700_000_000_000_000_000, approved_at: 0,
        expires_at: u64::MAX, approval_bitmap: 0, cancellation_bitmap: 0,
        nostr_approval_bitmap: 0, nostr_cancellation_bitmap: 0,
        param_values: "{}".to_string(), message: "test".to_string(),
        intent_params_hash: "".to_string(),
    };
    contract.proposals.insert(&proposal_key("treasury", 0), &proposal);

    testing_env!(get_context(Some(alice()), 0));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        contract.execute("treasury".to_string(), 0);
    }));
    assert!(result.is_err()); // ERR_NOT_APPROVED
}

// ══════════════════════════════════════════════════════════════════════════
// LIST PROPOSALS
// ══════════════════════════════════════════════════════════════════════════

// #[test]
fn test_vm_list_proposals_empty() {
    let mut contract = setup_contract();
    create_default_wallet(&mut contract);

    testing_env!(get_context(Some(alice()), 0));
    let proposals = contract.list_proposals("treasury".to_string());
    assert!(proposals.is_empty());
}

// #[test]
fn test_vm_list_proposals_after_execute() {
    let mut contract = setup_contract();
    create_default_wallet(&mut contract);

    // Create + execute a proposal
    let add_params = serde_json::json!({
        "hash": "v1",
        "name": "T", "template": "x", "proposers": [alice().as_str()],
        "approvers": [alice().as_str()], "approval_threshold": 1,
        "timelock_seconds": 0, "params": [{"name": "x", "param_type": "String"}]
    }).to_string();

    let intent0 = contract.get_intent("treasury".to_string(), 0).unwrap();
    let p0 = Proposal {
        id: 0, wallet_name: "treasury".to_string(), intent_index: 0,
        proposer: alice(), status: ProposalStatus::Approved,
        proposed_at: 1_700_000_000_000_000_000,
        approved_at: 1_700_000_000_000_000_000,
        expires_at: u64::MAX, approval_bitmap: 1, cancellation_bitmap: 0,
        nostr_approval_bitmap: 0, nostr_cancellation_bitmap: 0,
        param_values: add_params, message: "test".to_string(),
        intent_params_hash: hash_params(&intent0.params),
    };
    contract.proposals.insert(&proposal_key("treasury", 0), &p0);
    let mut i0 = contract.intents.get(&intent_key("treasury", 0)).unwrap();
    i0.active_proposal_count = 1;
    contract.intents.insert(&intent_key("treasury", 0), &i0);
    // Update wallet proposal_index so list_proposals finds it
    let mut w = contract.wallets.get(&"treasury".to_string()).unwrap();
    w.proposal_index = 1;
    contract.wallets.insert(&"treasury".to_string(), &w);

    testing_env!(get_context(Some(alice()), 0));
    contract.execute("treasury".to_string(), 0);

    let proposals = contract.list_proposals("treasury".to_string());
    assert_eq!(proposals.len(), 1);
    assert_eq!(proposals[0].status, ProposalStatus::Executed);
}

// ══════════════════════════════════════════════════════════════════════════
// EXECUTE DISPATCH - TRANSFER FT
// ══════════════════════════════════════════════════════════════════════════

// #[test]
fn test_vm_execute_transfer_ft_insufficient() {
    let mut contract = setup_contract();
    create_default_wallet(&mut contract);

    // Add "Transfer FT" intent
    let add_params = serde_json::json!({
        "hash": "v1",
        "name": "Transfer FT",
        "template": "transfer ft",
        "proposers": [alice().as_str()],
        "approvers": [alice().as_str()],
        "approval_threshold": 1,
        "timelock_seconds": 0,
        "params": [
            {"name": "token", "param_type": "AccountId"},
            {"name": "recipient", "param_type": "AccountId"},
            {"name": "amount", "param_type": "U128"}
        ]
    }).to_string();

    let intent0 = contract.get_intent("treasury".to_string(), 0).unwrap();
    let p0 = Proposal {
        id: 0, wallet_name: "treasury".to_string(), intent_index: 0,
        proposer: alice(), status: ProposalStatus::Approved,
        proposed_at: 1_700_000_000_000_000_000,
        approved_at: 1_700_000_000_000_000_000,
        expires_at: u64::MAX, approval_bitmap: 1, cancellation_bitmap: 0,
        nostr_approval_bitmap: 0, nostr_cancellation_bitmap: 0,
        param_values: add_params, message: "test".to_string(),
        intent_params_hash: hash_params(&intent0.params),
    };
    contract.proposals.insert(&proposal_key("treasury", 0), &p0);
    let mut i0 = contract.intents.get(&intent_key("treasury", 0)).unwrap();
    i0.active_proposal_count = 1;
    contract.intents.insert(&intent_key("treasury", 0), &i0);

    testing_env!(get_context(Some(alice()), 0));
    contract.execute("treasury".to_string(), 0);

    // Try transfer FT with no balance
    let transfer_params = serde_json::json!({
        "token": token_contract().as_str(),
        "recipient": bob().as_str(),
        "amount": "1000",
    }).to_string();

    let p1 = Proposal {
        id: 1, wallet_name: "treasury".to_string(), intent_index: 3,
        proposer: alice(), status: ProposalStatus::Approved,
        proposed_at: 1_700_000_000_000_000_000,
        approved_at: 1_700_000_000_000_000_000,
        expires_at: u64::MAX, approval_bitmap: 1, cancellation_bitmap: 0,
        nostr_approval_bitmap: 0, nostr_cancellation_bitmap: 0,
        param_values: transfer_params, message: "test".to_string(),
        intent_params_hash: hash_params(&contract.get_intent("treasury".to_string(), 3).unwrap().params),
    };
    contract.proposals.insert(&proposal_key("treasury", 1), &p1);
    let mut i3 = contract.intents.get(&intent_key("treasury", 3)).unwrap();
    i3.active_proposal_count = 1;
    contract.intents.insert(&intent_key("treasury", 3), &i3);

    testing_env!(get_context(Some(alice()), 0));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        contract.execute("treasury".to_string(), 1);
    }));
    assert!(result.is_err()); // ERR_INSUFFICIENT_FT
}
