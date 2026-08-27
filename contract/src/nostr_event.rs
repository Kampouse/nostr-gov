/// Nostr event signature verification for NIP-46 compatibility.
///
/// A nostr event's `id` = SHA256(compact_json([0, pubkey, created_at, kind, tags, content])).
/// The event's `sig` = BIP-340 schnorr_sign(event_id, sk).
///
/// We verify by:
/// 1. Reconstructing the event serialization from the fields
/// 2. Checking SHA256(serialization) == event_id
/// 3. Verifying schnorr(pubkey, sig, event_id_bytes)
/// 4. Checking content matches the expected message

use crate::schnorr::schnorr_verify;
use crate::message::hex_decode;
use sha2::{Sha256, Digest};

/// Governance event kind — used for multisig approvals via NIP-46.
pub const GOVERNANCE_KIND: u32 = 37500;

/// Verify a signed nostr event and return the event ID as bytes.
/// Returns the 32-byte event ID on success, panics on failure.
pub fn verify_event(
    pubkey_hex: &str,
    event_id_hex: &str,
    created_at: u64,
    kind: u32,
    tags_json: &str,
    content: &str,
    sig_hex: &str,
) -> [u8; 32] {
    // Parse inputs
    assert_eq!(pubkey_hex.len(), 64, "ERR_EVENT_PK_LEN");
    assert_eq!(event_id_hex.len(), 64, "ERR_EVENT_ID_LEN");
    assert_eq!(sig_hex.len(), 128, "ERR_EVENT_SIG_LEN");
    assert_eq!(kind, GOVERNANCE_KIND, "ERR_EVENT_KIND: expected 37500");

    let pk: [u8; 32] = hex_decode(pubkey_hex).try_into().unwrap_or_else(|_| {
        env::panic_str("ERR_EVENT_PK_DECODE")
    });
    let sig: [u8; 64] = hex_decode(sig_hex).try_into().unwrap_or_else(|_| {
        env::panic_str("ERR_EVENT_SIG_DECODE")
    });
    let expected_id: [u8; 32] = hex_decode(event_id_hex).try_into().unwrap_or_else(|_| {
        env::panic_str("ERR_EVENT_ID_DECODE")
    });

    // Reconstruct event serialization: [0, "<pubkey>", <created_at>, <kind>, <tags>, "<content>"]
    // Use serde_json for correct escaping of content strings.
    let tags_val: serde_json::Value = serde_json::from_str(tags_json).unwrap_or_else(|_| {
        env::panic_str("ERR_EVENT_TAGS_JSON")
    });
    let event_arr = serde_json::json!([0, pubkey_hex, created_at, kind, tags_val, content]);
    let serialized = serde_json::to_string(&event_arr)
        .unwrap_or_else(|_| env::panic_str("ERR_EVENT_SERIALIZE"));

    // Verify event ID = SHA256(serialization)
    let computed_id: [u8; 32] = Sha256::digest(serialized.as_bytes()).into();
    assert_eq!(computed_id, expected_id, "ERR_EVENT_ID_MISMATCH");

    // Verify schnorr signature over the event ID
    assert!(schnorr_verify(&pk, &sig, &expected_id), "ERR_EVENT_SIG_INVALID");

    computed_id
}

use near_sdk::env;
