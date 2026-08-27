//! Message building for clear-signing.
//!
//! All messages follow the format:
//!   `expires <timestamp>: <action> <content> | wallet: <name> proposal: <index>`
//!
//! Signers see exactly what they're approving — no opaque transaction bytes.

use crate::*;

/// Build a human-readable message for signing
pub fn build_message(
    wallet_name: &str,
    proposal_index: u64,
    expires_at: u64,
    action: &str,
    intent: &Intent,
    params: &serde_json::Value,
) -> String {
    let content = match intent.intent_type {
        IntentType::AddIntent => {
            let hash = params.get("hash").and_then(|v| v.as_str()).unwrap_or("unknown");
            format!("add intent definition_hash: {}", hash)
        }
        IntentType::RemoveIntent => {
            let idx = params.get("index").and_then(|v| v.as_u64()).unwrap_or(0);
            format!("remove intent {}", idx)
        }
        IntentType::UpdateIntent => {
            let idx = params.get("index").and_then(|v| v.as_u64()).unwrap_or(0);
            format!("update intent {}", idx)
        }
        IntentType::Custom => intent.render_template(params),
        IntentType::Transfer => {
            let amount = params.get("amount").and_then(|v| v.as_str()).unwrap_or("?");
            let recipient = params.get("recipient").and_then(|v| v.as_str()).unwrap_or("?");
            format!("transfer {} to {}", amount, recipient)
        }
        IntentType::Deposit => "deposit NEAR to wallet".to_string(),
        IntentType::Call => {
            let receiver = params.get("receiver_id").and_then(|v| v.as_str()).unwrap_or("?");
            let method = params.get("method_name").and_then(|v| v.as_str()).unwrap_or("?");
            format!("call {}.{}({})", receiver, method, 
                params.get("args").and_then(|v| v.as_str()).unwrap_or("").chars().take(32).collect::<String>())
        }
    };

    // Convert nanoseconds to ISO-ish timestamp
    let expires_secs = expires_at / 1_000_000_000;
    let expires_nanos = expires_at % 1_000_000_000;
    let expires_display = format!("{}.{:09}", expires_secs, expires_nanos);

    // Binding the contract account id into the signed message prevents
    // cross-contract / cross-chain replay of approval signatures.
    format!(
        "expires {}: {} {} | wallet: {} proposal: {} | contract: {}",
        expires_display, action, content, wallet_name, proposal_index,
        env::current_account_id()
    )
}

/// Verify an ed25519 signature over a message
#[allow(dead_code)]
pub fn verify_signature(public_key: &str, signature_hex: &str, message: &str) {
    // Parse public key (strip "ed25519:" prefix if present)
    let pk_str = public_key.strip_prefix("ed25519:").unwrap_or(public_key);
    let pk_bytes = hex_decode(pk_str);
    assert_eq!(pk_bytes.len(), 32, "Invalid public key length");
    let pk: [u8; 32] = pk_bytes.try_into().unwrap();

    // Parse signature
    let sig_bytes = hex_decode(signature_hex);
    assert_eq!(sig_bytes.len(), 64, "Invalid signature length");
    let sig: [u8; 64] = sig_bytes.try_into().unwrap();

    // Verify using NEAR's built-in ed25519 verification
    let valid = env::ed25519_verify(&sig, message.as_bytes(), &pk);
    assert!(valid, "Invalid signature: the message was not signed by this key");
}

/// Non-panicking schnorr verify — returns false instead of panicking.
/// Uses hand-rolled BIP-340 Jacobian verifier (no k256 dependency on-chain).
pub fn try_schnorr_verify(pubkey_hex: &str, signature_hex: &str, message: &str) -> bool {
    use sha2::{Sha256, Digest};

    if pubkey_hex.len() != 64 || signature_hex.len() != 128 { return false; }
    if !pubkey_hex.chars().all(|c| c.is_ascii_hexdigit()) { return false; }
    if !signature_hex.chars().all(|c| c.is_ascii_hexdigit()) { return false; }

    let pk_bytes = hex_decode(pubkey_hex);
    let Ok(pk_arr): Result<[u8; 32], _> = pk_bytes.as_slice().try_into() else { return false; };
    let sig_bytes = hex_decode(signature_hex);
    let Ok(sig_arr): Result<[u8; 64], _> = sig_bytes.as_slice().try_into() else { return false; };
    let msg_hash = Sha256::digest(message.as_bytes());
    let msg_arr: [u8; 32] = msg_hash.into();
    crate::schnorr::schnorr_verify(&pk_arr, &sig_arr, &msg_arr)
}

/// Verify a BIP-340 schnorr signature (used by Nostr).
/// `pubkey_hex` is the 32-byte x-only public key (npub, hex-encoded).
/// `signature_hex` is the 64-byte schnorr signature (hex-encoded).
/// `message` is the clear-sign text that was signed.
pub fn verify_schnorr_signature(pubkey_hex: &str, signature_hex: &str, message: &str) {
    use sha2::{Sha256, Digest};

    let pk_bytes = hex_decode(pubkey_hex);
    assert_eq!(pk_bytes.len(), 32, "Invalid schnorr public key length (expected 32 bytes)");

    let sig_bytes = hex_decode(signature_hex);
    assert_eq!(sig_bytes.len(), 64, "Invalid schnorr signature length (expected 64 bytes)");

    let pk_arr: [u8; 32] = pk_bytes.as_slice().try_into().unwrap();
    let sig_arr: [u8; 64] = sig_bytes.as_slice().try_into().unwrap();

    // Hash the message with SHA256 (same as Nostr does for event content signing)
    let msg_hash = Sha256::digest(message.as_bytes());
    let msg_arr: [u8; 32] = msg_hash.into();

    assert!(crate::schnorr::schnorr_verify(&pk_arr, &sig_arr, &msg_arr),
        "Invalid schnorr signature: verification failed");
}

pub fn hex_decode(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&s[i..i + 2], 16)
                .unwrap_or_else(|_| panic!("Invalid hex at position {}", i))
        })
        .collect()
}

#[allow(dead_code)]
pub fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;



    /// Embedded selftest pair generated by k256 (sk = [0xaa; 32]) — used by
    /// native k256 — proves the schnorr path used on-chain works with the same crate.
    #[test]
    fn test_try_schnorr_verify_selftest_pair() {
        assert!(try_schnorr_verify(
            "6a04ab98d9e4774ad806e302dddeb63bea16b5cb5f223ee77478e861bb583eb3",
            "934f079b06b4c39cda8d16cefcbbb4ce34e5e7a18a6f1440c3a342466f5276da7ce1b5a07e1d376639e366eb997ec19391dc68049bc78f15b2ed9f79bdada8aa",
            "test"
        ));
        // wrong message must fail
        assert!(!try_schnorr_verify(
            "6a04ab98d9e4774ad806e302dddeb63bea16b5cb5f223ee77478e861bb583eb3",
            "934f079b06b4c39cda8d16cefcbbb4ce34e5e7a18a6f1440c3a342466f5276da7ce1b5a07e1d376639e366eb997ec19391dc68049bc78f15b2ed9f79bdada8aa",
            "wrong"
        ));
    }

    fn make_test_intent() -> Intent {
        Intent {
            wallet_name: "treasury".to_string(),
            index: 3,
            intent_type: IntentType::Custom,
            name: "Transfer NEAR".to_string(),
            template: "transfer {amount} NEAR to {recipient}".to_string(),
            proposers: vec![],
            approvers: vec![],
            nostr_approvers: vec![],
            approval_threshold: 2,
            cancellation_threshold: 2,
            timelock_seconds: 0,
            params: vec![
                ParamDef {
                    name: "amount".to_string(),
                    param_type: ParamType::U128,
                    max_value: Some(U128(10_000_000_000_000_000_000_000_000)),
                },
                ParamDef {
                    name: "recipient".to_string(),
                    param_type: ParamType::AccountId,
                    max_value: None,
                },
            ],
            execution_gas_tgas: 50,
            active: true,
            active_proposal_count: 0,
        }
    }

    #[test]
    fn test_build_custom_message() {
        let intent = make_test_intent();
        let params = serde_json::json!({
            "amount": "1000000000000000000000000",
            "recipient": "bob.near"
        });

        let msg = build_message("treasury", 42, 1893456000_000_000_000, "propose", &intent, &params);

        assert!(msg.contains("transfer 1000000000000000000000000 NEAR to bob.near"));
        assert!(msg.contains("wallet: treasury proposal: 42"));
        assert!(msg.contains("expires"));
        assert!(msg.contains("propose"));
    }

    #[test]
    fn test_build_message_all_actions() {
        let intent = make_test_intent();
        let params = serde_json::json!({
            "amount": "1000000000000000000000000",
            "recipient": "bob.near"
        });

        for action in &["propose", "approve", "cancel", "amend"] {
            let msg = build_message("w", 0, 1000000000_000_000_000, action, &intent, &params);
            assert!(msg.starts_with("expires"));
            assert!(msg.contains(action));
        }
    }

    #[test]
    fn test_hex_encode_decode() {
        let original = vec![0x00, 0x01, 0xfe, 0xff];
        let encoded = hex_encode(&original);
        assert_eq!(encoded, "0001feff");
        let decoded = hex_decode(&encoded);
        assert_eq!(decoded, original);
    }
}
