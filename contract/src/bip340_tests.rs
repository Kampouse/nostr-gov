//! Official BIP-340 test vectors — Tier-1 hardening.
//!
//! Source: https://github.com/bitcoin/bips/blob/master/bip-0340/test-vectors.csv
//! (19 vectors, including the 2022-12 variable-length message additions).
//!
//! These exercise the EXACT verification primitive used on-chain:
//! `VerifyingKey::verify_raw(m, sig)` (k256 0.13.4) — the same call
//! `message::try_schnorr_verify` makes after the nostr-style
//! `SHA256(clear_sign_text)` pre-hash. BIP-340 uses the message bytes
//! directly in the `BIP0340/challenge` tagged hash, so a 32-byte nostr
//! event-id / clear-sign digest and the vector messages flow through
//! identical code. Parse guards mirror `try_schnorr_verify` exactly
//! (length + hexdigit checks, x-only key parse, signature parse).

use crate::message::hex_decode;
use k256::schnorr::{Signature, VerifyingKey};

struct Vector {
    idx: u32,
    sk: &'static str, // empty when the vector omits it
    pk: &'static str,
    msg: &'static str,
    sig: &'static str,
    ok: bool,
    comment: &'static str,
}

const VECTORS: &[Vector] = &[
    Vector { idx: 0,
        sk:  "0000000000000000000000000000000000000000000000000000000000000003",
        pk:  "F9308A019258C31049344F85F89D5229B531C845836F99B08601F113BCE036F9",
        msg: "0000000000000000000000000000000000000000000000000000000000000000",
        sig: "E907831F80848D1069A5371B402410364BDF1C5F8307B0084C55F1CE2DCA821525F66A4A85EA8B71E482A74F382D2CE5EBEEE8FDB2172F477DF4900D310536C0",
        ok: true, comment: "" },
    Vector { idx: 1,
        sk:  "B7E151628AED2A6ABF7158809CF4F3C762E7160F38B4DA56A784D9045190CFEF",
        pk:  "DFF1D77F2A671C5F36183726DB2341BE58FEAE1DA2DECED843240F7B502BA659",
        msg: "243F6A8885A308D313198A2E03707344A4093822299F31D0082EFA98EC4E6C89",
        sig: "6896BD60EEAE296DB48A229FF71DFE071BDE413E6D43F917DC8DCF8C78DE33418906D11AC976ABCCB20B091292BFF4EA897EFCB639EA871CFA95F6DE339E4B0A",
        ok: true, comment: "" },
    Vector { idx: 2,
        sk:  "C90FDAA22168C234C4C6628B80DC1CD129024E088A67CC74020BBEA63B14E5C9",
        pk:  "DD308AFEC5777E13121FA72B9CC1B7CC0139715309B086C960E18FD969774EB8",
        msg: "7E2D58D8B3BCDF1ABADEC7829054F90DDA9805AAB56C77333024B9D0A508B75C",
        sig: "5831AAEED7B44BB74E5EAB94BA9D4294C49BCF2A60728D8B4C200F50DD313C1BAB745879A5AD954A72C45A91C3A51D3C7ADEA98D82F8481E0E1E03674A6F3FB7",
        ok: true, comment: "" },
    Vector { idx: 3,
        sk:  "0B432B2677937381AEF05BB02A66ECD012773062CF3FA2549E44F58ED2401710",
        pk:  "25D1DFF95105F5253C4022F628A996AD3A0D95FBF21D468A1B33F8C160D8F517",
        msg: "FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF",
        sig: "7EB0509757E246F19449885651611CB965ECC1A187DD51B64FDA1EDC9637D5EC97582B9CB13DB3933705B32BA982AF5AF25FD78881EBB32771FC5922EFC66EA3",
        ok: true, comment: "test fails if msg is reduced modulo p or n" },
    Vector { idx: 4,
        sk:  "",
        pk:  "D69C3509BB99E412E68B0FE8544E72837DFA30746D8BE2AA65975F29D22DC7B9",
        msg: "4DF3C3F68FCC83B27E9D42C90431A72499F17875C81A599B566C9889B9696703",
        sig: "00000000000000000000003B78CE563F89A0ED9414F5AA28AD0D96D6795F9C6376AFB1548AF603B3EB45C9F8207DEE1060CB71C04E80F593060B07D28308D7F4",
        ok: true, comment: "" },
    Vector { idx: 5,
        sk:  "",
        pk:  "EEFDEA4CDB677750A420FEE807EACF21EB9898AE79B9768766E4FAA04A2D4A34",
        msg: "243F6A8885A308D313198A2E03707344A4093822299F31D0082EFA98EC4E6C89",
        sig: "6CFF5C3BA86C69EA4B7376F31A9BCB4F74C1976089B2D9963DA2E5543E17776969E89B4C5564D00349106B8497785DD7D1D713A8AE82B32FA79D5F7FC407D39B",
        ok: false, comment: "public key not on the curve" },
    Vector { idx: 6,
        sk:  "",
        pk:  "DFF1D77F2A671C5F36183726DB2341BE58FEAE1DA2DECED843240F7B502BA659",
        msg: "243F6A8885A308D313198A2E03707344A4093822299F31D0082EFA98EC4E6C89",
        sig: "FFF97BD5755EEEA420453A14355235D382F6472F8568A18B2F057A14602975563CC27944640AC607CD107AE10923D9EF7A73C643E166BE5EBEAFA34B1AC553E2",
        ok: false, comment: "has_even_y(R) is false" },
    Vector { idx: 7,
        sk:  "",
        pk:  "DFF1D77F2A671C5F36183726DB2341BE58FEAE1DA2DECED843240F7B502BA659",
        msg: "243F6A8885A308D313198A2E03707344A4093822299F31D0082EFA98EC4E6C89",
        sig: "1FA62E331EDBC21C394792D2AB1100A7B432B013DF3F6FF4F99FCB33E0E1515F28890B3EDB6E7189B630448B515CE4F8622A954CFE545735AAEA5134FCCDB2BD",
        ok: false, comment: "negated message" },
    Vector { idx: 8,
        sk:  "",
        pk:  "DFF1D77F2A671C5F36183726DB2341BE58FEAE1DA2DECED843240F7B502BA659",
        msg: "243F6A8885A308D313198A2E03707344A4093822299F31D0082EFA98EC4E6C89",
        sig: "6CFF5C3BA86C69EA4B7376F31A9BCB4F74C1976089B2D9963DA2E5543E177769961764B3AA9B2FFCB6EF947B6887A226E8D7C93E00C5ED0C1834FF0D0C2E6DA6",
        ok: false, comment: "negated s value" },
    Vector { idx: 9,
        sk:  "",
        pk:  "DFF1D77F2A671C5F36183726DB2341BE58FEAE1DA2DECED843240F7B502BA659",
        msg: "243F6A8885A308D313198A2E03707344A4093822299F31D0082EFA98EC4E6C89",
        sig: "0000000000000000000000000000000000000000000000000000000000000000123DDA8328AF9C23A94C1FEECFD123BA4FB73476F0D594DCB65C6425BD186051",
        ok: false, comment: "sG - eP is infinite (has_even_y(inf)=true, x(inf)=0 case)" },
    Vector { idx: 10,
        sk:  "",
        pk:  "DFF1D77F2A671C5F36183726DB2341BE58FEAE1DA2DECED843240F7B502BA659",
        msg: "243F6A8885A308D313198A2E03707344A4093822299F31D0082EFA98EC4E6C89",
        sig: "00000000000000000000000000000000000000000000000000000000000000017615FBAF5AE28864013C099742DEADB4DBA87F11AC6754F93780D5A1837CF197",
        ok: false, comment: "sG - eP is infinite (has_even_y(inf)=true, x(inf)=1 case)" },
    Vector { idx: 11,
        sk:  "",
        pk:  "DFF1D77F2A671C5F36183726DB2341BE58FEAE1DA2DECED843240F7B502BA659",
        msg: "243F6A8885A308D313198A2E03707344A4093822299F31D0082EFA98EC4E6C89",
        sig: "4A298DACAE57395A15D0795DDBFD1DCB564DA82B0F269BC70A74F8220429BA1D69E89B4C5564D00349106B8497785DD7D1D713A8AE82B32FA79D5F7FC407D39B",
        ok: false, comment: "sig[0:32] is not an X coordinate on the curve" },
    Vector { idx: 12,
        sk:  "",
        pk:  "DFF1D77F2A671C5F36183726DB2341BE58FEAE1DA2DECED843240F7B502BA659",
        msg: "243F6A8885A308D313198A2E03707344A4093822299F31D0082EFA98EC4E6C89",
        sig: "FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFEFFFFFC2F69E89B4C5564D00349106B8497785DD7D1D713A8AE82B32FA79D5F7FC407D39B",
        ok: false, comment: "sig[0:32] is equal to field size" },
    Vector { idx: 13,
        sk:  "",
        pk:  "DFF1D77F2A671C5F36183726DB2341BE58FEAE1DA2DECED843240F7B502BA659",
        msg: "243F6A8885A308D313198A2E03707344A4093822299F31D0082EFA98EC4E6C89",
        sig: "6CFF5C3BA86C69EA4B7376F31A9BCB4F74C1976089B2D9963DA2E5543E177769FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFEBAAEDCE6AF48A03BBFD25E8CD0364141",
        ok: false, comment: "sig[32:64] is equal to curve order" },
    Vector { idx: 14,
        sk:  "",
        pk:  "FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFEFFFFFC30",
        msg: "243F6A8885A308D313198A2E03707344A4093822299F31D0082EFA98EC4E6C89",
        sig: "6CFF5C3BA86C69EA4B7376F31A9BCB4F74C1976089B2D9963DA2E5543E17776969E89B4C5564D00349106B8497785DD7D1D713A8AE82B32FA79D5F7FC407D39B",
        ok: false, comment: "public key is not a valid X coordinate because it exceeds the field size" },
    Vector { idx: 15,
        sk:  "0340034003400340034003400340034003400340034003400340034003400340",
        pk:  "778CAA53B4393AC467774D09497A87224BF9FAB6F6E68B23086497324D6FD117",
        msg: "",
        sig: "71535DB165ECD9FBBC046E5FFAEA61186BB6AD436732FCCC25291A55895464CF6069CE26BF03466228F19A3A62DB8A649F2D560FAC652827D1AF0574E427AB63",
        ok: true, comment: "message of size 0 (added 2022-12)" },
    Vector { idx: 16,
        sk:  "0340034003400340034003400340034003400340034003400340034003400340",
        pk:  "778CAA53B4393AC467774D09497A87224BF9FAB6F6E68B23086497324D6FD117",
        msg: "11",
        sig: "08A20A0AFEF64124649232E0693C583AB1B9934AE63B4C3511F3AE1134C6A303EA3173BFEA6683BD101FA5AA5DBC1996FE7CACFC5A577D33EC14564CEC2BACBF",
        ok: true, comment: "message of size 1 (added 2022-12)" },
    Vector { idx: 17,
        sk:  "0340034003400340034003400340034003400340034003400340034003400340",
        pk:  "778CAA53B4393AC467774D09497A87224BF9FAB6F6E68B23086497324D6FD117",
        msg: "0102030405060708090A0B0C0D0E0F1011",
        sig: "5130F39A4059B43BC7CAC09A19ECE52B5D8699D1A71E3C52DA9AFDB6B50AC370C4A482B77BF960F8681540E25B6771ECE1E5A37FD80E5A51897C5566A97EA5A5",
        ok: true, comment: "message of size 17 (added 2022-12)" },
    Vector { idx: 18,
        sk:  "0340034003400340034003400340034003400340034003400340034003400340",
        pk:  "778CAA53B4393AC467774D09497A87224BF9FAB6F6E68B23086497324D6FD117",
        msg: "99999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999",
        sig: "403B12B0D8555A344175EA7EC746566303321E5DBFA8BE6F091635163ECA79A8585ED3E3170807E7C03B720FC54C7B23897FCBA0E9D0B4A06894CFD249F22367",
        ok: true, comment: "message of size 100 (added 2022-12)" },
];

/// Mirror of `message::try_schnorr_verify`'s parse + verify flow, with the
/// BIP-340 vector message passed directly as the signed message (the contract
/// passes a 32-byte SHA256 digest; BIP-340 challenge semantics are identical).
fn verify_bip340(pk_hex: &str, msg_hex: &str, sig_hex: &str) -> bool {
    // same input guards as the on-chain path
    if pk_hex.len() != 64 || sig_hex.len() != 128 {
        return false;
    }
    if !pk_hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return false;
    }
    if !sig_hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return false;
    }

    let pk_bytes = hex_decode(pk_hex);
    let Ok(pk_arr): Result<[u8; 32], _> = pk_bytes.as_slice().try_into() else {
        return false;
    };
    let Ok(pk) = VerifyingKey::from_bytes(&pk_arr) else {
        return false;
    };
    let sig_bytes = hex_decode(sig_hex);
    let Ok(sig) = Signature::try_from(&sig_bytes[..]) else {
        return false;
    };
    let msg_bytes = hex_decode(msg_hex);
    pk.verify_raw(&msg_bytes, &sig).is_ok()
}

#[test]
fn bip340_official_vectors_all_19() {
    for v in VECTORS {
        let got = verify_bip340(v.pk, v.msg, v.sig);
        let label = if v.comment.is_empty() {
            format!("vector {}", v.idx)
        } else {
            format!("vector {} — {}", v.idx, v.comment)
        };
        assert_eq!(
            got, v.ok,
            "{label}: expected verification result {}, got {}",
            v.ok, got
        );
    }
}

/// For every vector that ships a secret key (0-3), derive the x-only public
/// key the same way nostr wallets do (`sk · G`, x-coordinate) and confirm it
/// matches the vector — proving our `VerifyingKey::from_bytes` accepts the
/// pubkeys nostr tooling derives.
#[test]
fn bip340_secret_key_derivation_matches_vector_pubkeys() {
    use k256::schnorr::SigningKey;

    // Only vectors 0-3 ship proper 32-byte secret keys; the 2022-12 vectors
    // (15-18) deliberately use a 33-byte sk — out of scope for derivation.
    for v in VECTORS.iter().filter(|v| v.sk.len() == 64) {
        let sk_bytes = hex_decode(v.sk);
        let Ok(sk_arr): Result<[u8; 32], _> = sk_bytes.as_slice().try_into() else {
            panic!("vector {}: bad sk hex", v.idx);
        };
        let signing = SigningKey::from_bytes(&sk_arr)
            .unwrap_or_else(|_| panic!("vector {}: SigningKey rejected sk", v.idx));
        let derived = hex_encode_lower(&signing.verifying_key().to_bytes());
        let expected = v.pk.to_lowercase();
        assert_eq!(
            derived, expected,
            "vector {}: derived pubkey {} != vector pubkey {}",
            v.idx, derived, expected
        );
    }
}

fn hex_encode_lower(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", *b)).collect()
}

/// Nostr tie-in: the contract pre-hashes the clear-sign text with SHA256 and
/// calls `verify_raw` — identical to k256's own `Verifier::verify` (which
/// SHA256s then calls verify_raw). Prove the equivalence on a round-trip:
/// verify via k256's high-level `verify(text)` must agree with the contract's
/// manual `Sha256 → verify_raw` on the same inputs. (Signature generated with
/// k256's signer — the same crate, sign-side counterpart of the verify path.)
#[test]
fn nostr_style_prehash_path_matches_k256_verifier() {
    use k256::ecdsa::signature::{Signer, Verifier};
    use k256::schnorr::SigningKey;
    use k256::sha2::{Digest, Sha256};

    let sk = SigningKey::from_bytes(&[0x42u8; 32]).unwrap();
    let pk_hex = hex_encode_lower(&sk.verifying_key().to_bytes());
    let text = "expires 1893456000.000000000: transfer 1 NEAR to bob.near | wallet: treasury proposal: 7 | contract: nmsig.vault.kampy.testnet";

    let sig = sk.sign(text.as_bytes());
    let sig_hex = hex_encode_lower(&sig.to_bytes());

    // contract path (message.rs): manual SHA256 then verify_raw
    let digest = Sha256::digest(text.as_bytes());
    let contract_style = {
        let pk_bytes = hex_decode(&pk_hex);
        let arr: [u8; 32] = pk_bytes.as_slice().try_into().unwrap();
        let vk = VerifyingKey::from_bytes(&arr).unwrap();
        let sig_bytes = hex_decode(&sig_hex);
        let s = Signature::try_from(&sig_bytes[..]).unwrap();
        vk.verify_raw(&digest, &s).is_ok()
    };
    // k256 high-level path
    let k256_style = sk
        .verifying_key()
        .verify(text.as_bytes(), &sig)
        .is_ok();

    assert!(contract_style, "contract-style verify must accept");
    assert!(k256_style, "k256 Verifier::verify must accept");
    // and a wrong message must fail on both paths
    let wrong = Sha256::digest(b"tampered");
    let vk = VerifyingKey::from_bytes(hex_decode(&pk_hex).as_slice().try_into().unwrap()).unwrap();
    assert!(vk.verify_raw(&wrong, &sig).is_err());
    assert!(vk.verify(b"tampered".as_ref(), &sig).is_err());
}
