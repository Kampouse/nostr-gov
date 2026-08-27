//! VMContext-based integration tests for clear-msig.
//!
//! Tests actual contract methods using near_sdk's test VM.
//! Covers: wallet CRUD, intent management, execution dispatch,
//! FT operations, balance tracking, timelock, and error paths.

use crate::*;
use k256::ecdsa::signature::hazmat::PrehashSigner;
use k256::schnorr::{SigningKey, VerifyingKey, Signature};
use near_sdk::test_utils::{accounts, VMContextBuilder};
use near_sdk::{testing_env, NearToken, VMContext};
use sha2::{Sha256, Digest};

// ── Test keypair ────────────────────────────────────────────────────────────

fn test_signing_key() -> SigningKey {
    SigningKey::from_bytes(&[0x42u8; 32]).unwrap()
}

fn test_npub_hex() -> String {
    let sk = test_signing_key();
    let vk = sk.verifying_key();
    let encoded = vk.to_bytes();
    hex::encode(encoded)
}

/// Sign the message string using BIP-340 schnorr and return hex sig.
fn sign_action(action: &str, expires_at: u64, nonce: u64) -> String {
    let contract_id = "test.near";
    let msg = format!(
        "expires {}.000000000: {} | nonce: {} | contract: {}",
        expires_at, action, nonce, contract_id
    );
    let msg_hash = Sha256::digest(msg.as_bytes());
    let sig: Signature = test_signing_key().sign_prehash(&msg_hash).unwrap();
    hex::encode(sig.to_bytes())
}

const EXPIRES_AT: u64 = 2_500_000_000_000_000_000;
const CONTRACT_ID: &str = "test.near";

fn alice() -> AccountId { accounts(0) }
fn bob() -> AccountId { accounts(1) }
fn token_contract() -> AccountId { accounts(2) }

fn get_context(predecessor: Option<AccountId>, deposit: u128) -> VMContext {
    let mut builder = VMContextBuilder::new();
    let pred = predecessor.unwrap_or_else(alice);
    builder.predecessor_account_id(pred.clone());
    builder.signer_account_id(pred.clone());
    builder.signer_account_pk(
        near_sdk::PublicKey::from_parts(near_sdk::CurveType::ED25519, vec![0u8; 32]).unwrap()
    );
    builder.current_account_id(CONTRACT_ID.parse().unwrap());
    if deposit > 0 {
        builder.attached_deposit(NearToken::from_yoctonear(deposit));
    }
    builder.block_timestamp(1_700_000_000_000_000_000);
    builder.build()
}

fn setup_contract() -> Contract {
    Contract::new(vec![test_npub_hex()])
}

/// Create a wallet with a valid owner sig.
fn create_default_wallet(contract: &mut Contract) {
    testing_env!(get_context(Some(alice()), STORAGE_DEPOSIT_YOCTO));
    let sig = sign_action("create_wallet:treasury", EXPIRES_AT, 0);
    contract.create_wallet("treasury".to_string(), sig, EXPIRES_AT, 0);
}

/// Insert a pre-approved proposal into state.
fn insert_approved_proposal(
    contract: &mut Contract,
    wallet: &str,
    id: u64,
    intent_index: u32,
    param_values: String,
) {
    let intent = contract.get_intent(wallet.to_string(), intent_index).unwrap();
    let proposal = Proposal {
        id, wallet_name: wallet.to_string(), intent_index,
        proposer: alice(), status: ProposalStatus::Approved,
        proposed_at: 1_700_000_000_000_000_000,
        approved_at: 1_700_000_000_000_000_000,
        expires_at: u64::MAX, approval_bitmap: 1, cancellation_bitmap: 0,
        nostr_approval_bitmap: 0, nostr_cancellation_bitmap: 0,
        param_values, message: "test".to_string(),
        intent_params_hash: hash_params(&intent.params),
    };
    contract.proposals.insert(&proposal_key(wallet, id), &proposal);
    let mut im = contract.intents.get(&intent_key(wallet, intent_index)).unwrap();
    im.active_proposal_count += 1;
    contract.intents.insert(&intent_key(wallet, intent_index), &im);
}

/// Execute a proposal with a valid owner sig. Nonce = 10 + id.
fn execute_proposal(contract: &mut Contract, wallet: &str, proposal_id: u64) {
    let nonce = 10 + proposal_id;
    let sig = sign_action(&format!("execute:{}:{}", wallet, proposal_id), EXPIRES_AT, nonce);
    testing_env!(get_context(Some(alice()), 0));
    contract.execute(wallet.to_string(), proposal_id, sig, EXPIRES_AT, nonce);
}

// ══════════════════════════════════════════════════════════════════════════
// WALLET MANAGEMENT
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_vm_create_wallet() {
    let mut contract = setup_contract();
    create_default_wallet(&mut contract);
    let wallet = contract.get_wallet("treasury".to_string()).unwrap();
    assert_eq!(wallet.owner, alice());
    assert_eq!(wallet.intent_index, 3);
    assert_eq!(wallet.storage_deposit, STORAGE_DEPOSIT_YOCTO);
}

#[test]
fn test_vm_create_wallet_insufficient_deposit() {
    let mut contract = setup_contract();
    testing_env!(get_context(Some(alice()), 100));
    let sig = sign_action("create_wallet:treasury", EXPIRES_AT, 0);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        contract.create_wallet("treasury".to_string(), sig, EXPIRES_AT, 0);
    }));
    assert!(result.is_err());
}

#[test]
fn test_vm_delete_wallet() {
    let mut contract = setup_contract();
    create_default_wallet(&mut contract);
    testing_env!(get_context(Some(alice()), 0));
    let sig = sign_action("delete_wallet:treasury", EXPIRES_AT, 1);
    contract.delete_wallet("treasury".to_string(), sig, EXPIRES_AT, 1);
    assert!(contract.get_wallet("treasury".to_string()).is_none());
}


#[test]
fn test_vm_transfer_ownership() {
    let mut contract = setup_contract();
    create_default_wallet(&mut contract);
    testing_env!(get_context(Some(alice()), 0));
    let sig = sign_action("transfer_ownership:treasury", EXPIRES_AT, 1);
    contract.transfer_ownership("treasury".to_string(), bob(), sig, EXPIRES_AT, 1);
    let wallet = contract.get_wallet("treasury".to_string()).unwrap();
    assert_eq!(wallet.owner, bob());
    let i0 = contract.get_intent("treasury".to_string(), 0).unwrap();
    assert!(i0.proposers.contains(&bob()));
    assert!(!i0.proposers.contains(&alice()));
}

// ══════════════════════════════════════════════════════════════════════════
// INTENT MANAGEMENT
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_vm_meta_intents_created() {
    let mut contract = setup_contract();
    create_default_wallet(&mut contract);
    let intents = contract.list_intents("treasury".to_string());
    assert_eq!(intents.len(), 3);
    assert_eq!(intents[0].name, "AddIntent");
    assert_eq!(intents[1].name, "RemoveIntent");
    assert_eq!(intents[2].name, "UpdateIntent");
    for i in &intents {
        assert!(i.proposers.contains(&alice()));
        assert!(i.approvers.contains(&alice()));
    }
}

#[test]
fn test_vm_add_remove_allowed_token() {
    let mut contract = setup_contract();
    create_default_wallet(&mut contract);
    testing_env!(get_context(Some(alice()), 0));
    let sig = sign_action("add_allowed_token:treasury", EXPIRES_AT, 1);
    contract.add_allowed_token("treasury".to_string(), token_contract(), sig, EXPIRES_AT, 1);
    let tokens = contract.get_allowed_tokens("treasury".to_string());
    assert_eq!(tokens.len(), 1);
    assert_eq!(tokens[0], token_contract());
    testing_env!(get_context(Some(alice()), 0));
    let sig2 = sign_action("remove_allowed_token:treasury", EXPIRES_AT, 2);
    contract.remove_allowed_token("treasury".to_string(), token_contract(), sig2, EXPIRES_AT, 2);
    let tokens2 = contract.get_allowed_tokens("treasury".to_string());
    assert!(tokens2.is_empty());
}

// ══════════════════════════════════════════════════════════════════════════
// EXECUTE DISPATCH - ADD INTENT
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_vm_execute_add_intent() {
    let mut contract = setup_contract();
    create_default_wallet(&mut contract);
    let add_params = serde_json::json!({
        "hash": "v1", "name": "Transfer NEAR",
        "template": "transfer {amount} yoctoNEAR to {recipient}",
        "proposers": [alice().as_str()], "approvers": [alice().as_str()],
        "approval_threshold": 1, "timelock_seconds": 0,
        "params": [{"name": "amount", "param_type": "U128"}, {"name": "recipient", "param_type": "AccountId"}]
    }).to_string();
    insert_approved_proposal(&mut contract, "treasury", 0, 0, add_params);
    execute_proposal(&mut contract, "treasury", 0);
    let new_intent = contract.get_intent("treasury".to_string(), 3).unwrap();
    assert_eq!(new_intent.name, "Transfer NEAR");
    assert_eq!(new_intent.intent_type, IntentType::Custom);
    assert_eq!(new_intent.params.len(), 2);
    assert!(new_intent.active);
    let wallet = contract.get_wallet("treasury".to_string()).unwrap();
    assert_eq!(wallet.intent_index, 4);
    let p = contract.get_proposal("treasury".to_string(), 0).unwrap();
    assert_eq!(p.status, ProposalStatus::Executed);
}

// ══════════════════════════════════════════════════════════════════════════
// EXECUTE DISPATCH - REMOVE INTENT
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_vm_execute_remove_intent() {
    let mut contract = setup_contract();
    create_default_wallet(&mut contract);
    let add_params = serde_json::json!({
        "hash": "v1", "name": "Test Intent", "template": "do {thing}",
        "proposers": [alice().as_str()], "approvers": [alice().as_str()],
        "approval_threshold": 1, "timelock_seconds": 0,
        "params": [{"name": "thing", "param_type": "String"}]
    }).to_string();
    insert_approved_proposal(&mut contract, "treasury", 0, 0, add_params);
    execute_proposal(&mut contract, "treasury", 0);
    assert!(contract.get_intent("treasury".to_string(), 3).unwrap().active);
    let remove_params = serde_json::json!({"index": 3}).to_string();
    insert_approved_proposal(&mut contract, "treasury", 1, 1, remove_params);
    execute_proposal(&mut contract, "treasury", 1);
    assert!(!contract.get_intent("treasury".to_string(), 3).unwrap().active);
}

// ══════════════════════════════════════════════════════════════════════════
// EXECUTE DISPATCH - UPDATE INTENT
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_vm_execute_update_intent() {
    let mut contract = setup_contract();
    create_default_wallet(&mut contract);
    let add_params = serde_json::json!({
        "hash": "v1", "name": "Old Name", "template": "do {thing}",
        "proposers": [alice().as_str()], "approvers": [alice().as_str()],
        "approval_threshold": 1, "timelock_seconds": 0,
        "params": [{"name": "thing", "param_type": "String"}]
    }).to_string();
    insert_approved_proposal(&mut contract, "treasury", 0, 0, add_params);
    execute_proposal(&mut contract, "treasury", 0);
    let update_params = serde_json::json!({
        "index": 3, "name": "New Name", "template": "do {thing} now", "approval_threshold": 2,
    }).to_string();
    insert_approved_proposal(&mut contract, "treasury", 1, 2, update_params);
    execute_proposal(&mut contract, "treasury", 1);
    let updated = contract.get_intent("treasury".to_string(), 3).unwrap();
    assert_eq!(updated.name, "New Name");
    assert_eq!(updated.template, "do {thing} now");
    assert_eq!(updated.approval_threshold, 2);
}

// ══════════════════════════════════════════════════════════════════════════
// EXECUTE DISPATCH - DEPOSIT NEAR
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_vm_execute_deposit_near() {
    let mut contract = setup_contract();
    create_default_wallet(&mut contract);
    let add_params = serde_json::json!({
        "hash": "v1", "name": "Deposit NEAR", "template": "deposit near",
        "proposers": [alice().as_str()], "approvers": [alice().as_str()],
        "approval_threshold": 1, "timelock_seconds": 0,
        "params": [{"name": "deposit_note", "param_type": "String"}]
    }).to_string();
    insert_approved_proposal(&mut contract, "treasury", 0, 0, add_params);
    execute_proposal(&mut contract, "treasury", 0);
    let deposit_amount = 1_000_000_000_000_000_000_000_000u128;
    let deposit_params = serde_json::json!({"deposit_note": "test"}).to_string();
    insert_approved_proposal(&mut contract, "treasury", 1, 3, deposit_params);
    let nonce = 20;
    let sig = sign_action(&format!("execute:treasury:1"), EXPIRES_AT, nonce);
    testing_env!(get_context(Some(alice()), deposit_amount));
    contract.execute("treasury".to_string(), 1, sig, EXPIRES_AT, nonce);
    let near_bal = contract.get_wallet_near_balance("treasury".to_string());
    assert_eq!(near_bal.0, deposit_amount);
}

// ══════════════════════════════════════════════════════════════════════════
// EXECUTE DISPATCH - TRANSFER NEAR (insufficient balance)
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_vm_execute_transfer_near_insufficient() {
    let mut contract = setup_contract();
    create_default_wallet(&mut contract);
    let add_params = serde_json::json!({
        "hash": "v1", "name": "Transfer NEAR",
        "template": "transfer {amount} to {recipient}",
        "proposers": [alice().as_str()], "approvers": [alice().as_str()],
        "approval_threshold": 1, "timelock_seconds": 0,
        "params": [{"name": "amount", "param_type": "U128"}, {"name": "recipient", "param_type": "AccountId"}]
    }).to_string();
    insert_approved_proposal(&mut contract, "treasury", 0, 0, add_params);
    execute_proposal(&mut contract, "treasury", 0);
    let transfer_params = serde_json::json!({
        "amount": "1000000000000000000000000", "recipient": bob().as_str(),
    }).to_string();
    insert_approved_proposal(&mut contract, "treasury", 1, 3, transfer_params);
    let nonce = 20;
    let sig = sign_action(&format!("execute:treasury:1"), EXPIRES_AT, nonce);
    testing_env!(get_context(Some(alice()), 0));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        contract.execute("treasury".to_string(), 1, sig, EXPIRES_AT, nonce);
    }));
    assert!(result.is_err());
}

// ══════════════════════════════════════════════════════════════════════════
// TIMELOCK ENFORCEMENT
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_vm_timelock_blocks_execution() {
    let mut contract = setup_contract();
    create_default_wallet(&mut contract);
    let add_params = serde_json::json!({
        "hash": "v1", "name": "Timelocked", "template": "do {thing}",
        "proposers": [alice().as_str()], "approvers": [alice().as_str()],
        "approval_threshold": 1, "timelock_seconds": 0,
        "params": [{"name": "thing", "param_type": "String"}]
    }).to_string();
    insert_approved_proposal(&mut contract, "treasury", 0, 0, add_params);
    execute_proposal(&mut contract, "treasury", 0);
    let update_params = serde_json::json!({"index": 3, "timelock_seconds": 3600}).to_string();
    insert_approved_proposal(&mut contract, "treasury", 1, 2, update_params);
    execute_proposal(&mut contract, "treasury", 1);
    let exec_params = serde_json::json!({"thing": "test"}).to_string();
    insert_approved_proposal(&mut contract, "treasury", 2, 3, exec_params);
    let nonce = 20;
    let sig = sign_action(&format!("execute:treasury:2"), EXPIRES_AT, nonce);
    testing_env!(get_context(Some(alice()), 0));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        contract.execute("treasury".to_string(), 2, sig, EXPIRES_AT, nonce);
    }));
    assert!(result.is_err());
    let sig2 = sign_action(&format!("execute:treasury:2"), EXPIRES_AT, 21);
    let mut ctx = get_context(Some(alice()), 0);
    ctx.block_timestamp = 2_000_000_000_000_000_000;
    testing_env!(ctx);
    contract.execute("treasury".to_string(), 2, sig2, EXPIRES_AT, 21);
}

// ══════════════════════════════════════════════════════════════════════════
// PARAMS HASH VERIFICATION
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_vm_params_changed_blocks_execution() {
    let mut contract = setup_contract();
    create_default_wallet(&mut contract);
    let add_params = serde_json::json!({
        "hash": "v1", "name": "Test", "template": "do {thing}",
        "proposers": [alice().as_str()], "approvers": [alice().as_str()],
        "approval_threshold": 1, "timelock_seconds": 0,
        "params": [{"name": "thing", "param_type": "String"}]
    }).to_string();
    let proposal = Proposal {
        id: 0, wallet_name: "treasury".to_string(), intent_index: 0,
        proposer: alice(), status: ProposalStatus::Approved,
        proposed_at: 1_700_000_000_000_000_000,
        approved_at: 1_700_000_000_000_000_000,
        expires_at: u64::MAX, approval_bitmap: 1, cancellation_bitmap: 0,
        nostr_approval_bitmap: 0, nostr_cancellation_bitmap: 0,
        param_values: add_params, message: "test".to_string(),
        intent_params_hash: "wrong_hash".to_string(),
    };
    contract.proposals.insert(&proposal_key("treasury", 0), &proposal);
    let mut i0 = contract.intents.get(&intent_key("treasury", 0)).unwrap();
    i0.active_proposal_count = 1;
    contract.intents.insert(&intent_key("treasury", 0), &i0);
    let nonce = 20;
    let sig = sign_action(&format!("execute:treasury:0"), EXPIRES_AT, nonce);
    testing_env!(get_context(Some(alice()), 0));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        contract.execute("treasury".to_string(), 0, sig, EXPIRES_AT, nonce);
    }));
    assert!(result.is_err());
}

// ══════════════════════════════════════════════════════════════════════════
// FT ON TRANSFER
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_vm_ft_on_transfer() {
    let mut contract = setup_contract();
    create_default_wallet(&mut contract);
    testing_env!(get_context(Some(token_contract()), 0));
    let result = contract.ft_on_transfer(alice(), U128(1_000_000), "treasury".to_string());
    match result {
        PromiseOrValue::Value(v) => assert_eq!(v.0, 0),
        _ => panic!("Expected Value"),
    }
    let bal = contract.get_ft_balance("treasury".to_string(), token_contract());
    assert_eq!(bal.0, 1_000_000);
}

#[test]
fn test_vm_ft_on_transfer_blocked_token() {
    let mut contract = setup_contract();
    create_default_wallet(&mut contract);
    testing_env!(get_context(Some(alice()), 0));
    let usdt: AccountId = "usdt.tether-token.near".parse().unwrap();
    let sig = sign_action("add_allowed_token:treasury", EXPIRES_AT, 1);
    contract.add_allowed_token("treasury".to_string(), usdt, sig, EXPIRES_AT, 1);
    testing_env!(get_context(Some(token_contract()), 0));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        contract.ft_on_transfer(alice(), U128(1000), "treasury".to_string());
    }));
    assert!(result.is_err());
}

#[test]
fn test_vm_ft_cumulative_balance() {
    let mut contract = setup_contract();
    create_default_wallet(&mut contract);
    testing_env!(get_context(Some(token_contract()), 0));
    contract.ft_on_transfer(alice(), U128(500), "treasury".to_string());
    testing_env!(get_context(Some(token_contract()), 0));
    contract.ft_on_transfer(alice(), U128(300), "treasury".to_string());
    let bal = contract.get_ft_balance("treasury".to_string(), token_contract());
    assert_eq!(bal.0, 800);
}

// ══════════════════════════════════════════════════════════════════════════
// CLEANUP
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_vm_cleanup_executed() {
    let mut contract = setup_contract();
    create_default_wallet(&mut contract);
    let add_params = serde_json::json!({
        "hash": "v1", "name": "Test", "template": "do {thing}",
        "proposers": [alice().as_str()], "approvers": [alice().as_str()],
        "approval_threshold": 1, "timelock_seconds": 0,
        "params": [{"name": "thing", "param_type": "String"}]
    }).to_string();
    insert_approved_proposal(&mut contract, "treasury", 0, 0, add_params);
    execute_proposal(&mut contract, "treasury", 0);
    testing_env!(get_context(Some(alice()), 0));
    let sig = sign_action(&format!("cleanup:treasury:0"), EXPIRES_AT, 20);
    contract.cleanup("treasury".to_string(), 0, sig, EXPIRES_AT, 20);
    assert!(contract.get_proposal("treasury".to_string(), 0).is_none());
}

#[test]
fn test_vm_cleanup_active_blocked() {
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
    let sig = sign_action(&format!("cleanup:treasury:0"), EXPIRES_AT, 20);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        contract.cleanup("treasury".to_string(), 0, sig, EXPIRES_AT, 20);
    }));
    assert!(result.is_err());
}

// ══════════════════════════════════════════════════════════════════════════
// EVENT NONCE
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_vm_event_nonce_increments() {
    let mut contract = setup_contract();
    assert_eq!(contract.get_event_nonce(), 0);
    create_default_wallet(&mut contract);
    assert!(contract.get_event_nonce() > 0);
}

// ══════════════════════════════════════════════════════════════════════════
// EXECUTE NOT APPROVED
// ══════════════════════════════════════════════════════════════════════════

#[test]
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
    let nonce = 20;
    let sig = sign_action(&format!("execute:treasury:0"), EXPIRES_AT, nonce);
    testing_env!(get_context(Some(alice()), 0));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        contract.execute("treasury".to_string(), 0, sig, EXPIRES_AT, nonce);
    }));
    assert!(result.is_err());
}

// ══════════════════════════════════════════════════════════════════════════
// LIST PROPOSALS
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_vm_list_proposals_empty() {
    let mut contract = setup_contract();
    create_default_wallet(&mut contract);
    testing_env!(get_context(Some(alice()), 0));
    let proposals = contract.list_proposals("treasury".to_string());
    assert!(proposals.is_empty());
}

#[test]
fn test_vm_list_proposals_after_execute() {
    let mut contract = setup_contract();
    create_default_wallet(&mut contract);
    let add_params = serde_json::json!({
        "hash": "v1", "name": "T", "template": "x",
        "proposers": [alice().as_str()], "approvers": [alice().as_str()],
        "approval_threshold": 1, "timelock_seconds": 0,
        "params": [{"name": "x", "param_type": "String"}]
    }).to_string();
    insert_approved_proposal(&mut contract, "treasury", 0, 0, add_params);
    let mut w = contract.wallets.get(&"treasury".to_string()).unwrap();
    w.proposal_index = 1;
    contract.wallets.insert(&"treasury".to_string(), &w);
    execute_proposal(&mut contract, "treasury", 0);
    let proposals = contract.list_proposals("treasury".to_string());
    assert_eq!(proposals.len(), 1);
    assert_eq!(proposals[0].status, ProposalStatus::Executed);
}

// ══════════════════════════════════════════════════════════════════════════
// EXECUTE DISPATCH - TRANSFER FT
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_vm_execute_transfer_ft_insufficient() {
    let mut contract = setup_contract();
    create_default_wallet(&mut contract);
    let add_params = serde_json::json!({
        "hash": "v1", "name": "Transfer FT", "template": "transfer ft",
        "proposers": [alice().as_str()], "approvers": [alice().as_str()],
        "approval_threshold": 1, "timelock_seconds": 0,
        "params": [
            {"name": "token", "param_type": "AccountId"},
            {"name": "recipient", "param_type": "AccountId"},
            {"name": "amount", "param_type": "U128"}
        ]
    }).to_string();
    insert_approved_proposal(&mut contract, "treasury", 0, 0, add_params);
    execute_proposal(&mut contract, "treasury", 0);
    let transfer_params = serde_json::json!({
        "token": token_contract().as_str(), "recipient": bob().as_str(), "amount": "1000",
    }).to_string();
    insert_approved_proposal(&mut contract, "treasury", 1, 3, transfer_params);
    let nonce = 20;
    let sig = sign_action(&format!("execute:treasury:1"), EXPIRES_AT, nonce);
    testing_env!(get_context(Some(alice()), 0));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        contract.execute("treasury".to_string(), 1, sig, EXPIRES_AT, nonce);
    }));
    assert!(result.is_err());
}
