/// BIP-340 Schnorr verification — Jacobian coordinates, u64[4] LE field arithmetic.
/// Only deps: sha2 for tagged_hash.

use sha2::{Digest, Sha256};

const P: [u64; 4] = [
    0xFFFFFFFEFFFFFC2F, 0xFFFFFFFFFFFFFFFF, 0xFFFFFFFFFFFFFFFF, 0xFFFFFFFFFFFFFFFF,
];
const DELTA: u64 = 0x1000003D1;
const FE_ONE: [u64; 4] = [1, 0, 0, 0];
const P_MINUS_2: [u64; 4] = [
    0xFFFFFFFEFFFFFC2D, 0xFFFFFFFFFFFFFFFF, 0xFFFFFFFFFFFFFFFF, 0xFFFFFFFFFFFFFFFF,
];
const SQRT_EXP: [u64; 4] = [
    0xFFFFFFFFBFFFFF0C, 0xFFFFFFFFFFFFFFFF, 0xFFFFFFFFFFFFFFFF, 0x3FFFFFFFFFFFFFFF,
];
const GX: [u64; 4] = [0x59F2815B16F81798, 0x029BFCDB2DCE28D9, 0x55A06295CE870B07, 0x79BE667EF9DCBBAC];
const GY: [u64; 4] = [0x9C47D08FFB10D4B8, 0xFD17B448A6855419, 0x5DA4FBFC0E1108A8, 0x483ADA7726A3C465];
const N: [u64; 4] = [
    0xBFD25E8CD0364141, 0xBAAEDCE6AF48A03B, 0xFFFFFFFFFFFFFFFE, 0xFFFFFFFFFFFFFFFF,
];

// --- Field arithmetic helpers (from builtin_schnorr.rs, proven correct) ---

#[inline(always)]
fn add256(a: &[u64; 4], b: &[u64; 4]) -> ([u64; 4], u64) {
    let mut r = [0u64; 4]; let mut c = 0u64;
    for i in 0..4 { let (s1, c1) = a[i].overflowing_add(b[i]); let (s2, c2) = s1.overflowing_add(c); r[i] = s2; c = (c1 as u64) + (c2 as u64); }
    (r, c)
}

#[inline(always)]
fn sub256(a: &[u64; 4], b: &[u64; 4]) -> ([u64; 4], u64) {
    let mut r = [0u64; 4]; let mut borrow = 0u64;
    for i in 0..4 {
        let ai = a[i]; let bi = b[i].wrapping_add(borrow);
        borrow = if bi < borrow { 1 } else if ai < bi { 1 } else { 0 };
        r[i] = ai.wrapping_sub(bi);
    }
    (r, borrow)
}

#[inline(always)]
fn cond_sub_p(a: &[u64; 4]) -> [u64; 4] { let (r, b) = sub256(a, &P); if b == 0 { r } else { *a } }

#[inline(always)]
fn fe_add(a: &[u64; 4], b: &[u64; 4]) -> [u64; 4] {
    let (sum, c) = add256(a, b);
    if c > 0 { let (s, _) = add256(&sum, &[DELTA, 0, 0, 0]); cond_sub_p(&s) } else { cond_sub_p(&sum) }
}

#[inline(always)]
fn fe_sub(a: &[u64; 4], b: &[u64; 4]) -> [u64; 4] {
    let (d, b) = sub256(a, b);
    if b > 0 { let (s, b2) = sub256(&d, &[DELTA, 0, 0, 0]); let s = if b2 > 0 { add256(&s, &P).0 } else { s }; cond_sub_p(&s) } else { d }
}

#[inline(always)]
fn fe_mul(a: &[u64; 4], b: &[u64; 4]) -> [u64; 4] {
    let mask: u128 = (1u128 << 64) - 1;
    let mut r = [0u128; 8];
    for i in 0..4 { let mut carry = 0u128; for j in 0..4 { let prod = (a[i] as u128) * (b[j] as u128); let (s, o1) = r[i+j].overflowing_add(prod); let (s2, o2) = s.overflowing_add(carry); r[i+j] = s2 & mask; carry = ((o1 as u128 + o2 as u128) << 64) + (s2 >> 64); } r[i+4] += carry; }
    for k in 0..7 { r[k+1] += r[k] >> 64; r[k] &= mask; }
    let mut low = [r[0] as u64, r[1] as u64, r[2] as u64, r[3] as u64];
    let mut carry: u128 = 0;
    for i in 0..4 { let prod = (r[i+4] as u128) * (DELTA as u128) + (low[i] as u128) + carry; low[i] = prod as u64; carry = prod >> 64; }
    let cd = carry * (DELTA as u128);
    let lo_cd = cd as u64; let hi_cd = (cd >> 64) as u64;
    let (s, c) = add256(&low, &[lo_cd, hi_cd, 0, 0]);
    if c > 0 { let (s2, _) = add256(&s, &[DELTA, 0, 0, 0]); cond_sub_p(&s2) } else { cond_sub_p(&s) }
}

#[inline(always)]
fn fe_sqr(a: &[u64; 4]) -> [u64; 4] { fe_mul(a, a) }

fn fe_inv(a: &[u64; 4]) -> [u64; 4] {
    let mut r: [u64; 4] = [1, 0, 0, 0];
    let exp = P_MINUS_2;
    for w in 0..4 { let mut bits = exp[3-w]; for _ in 0..64 { r = fe_sqr(&r); if bits >> 63 != 0 { r = fe_mul(&r, a); } bits <<= 1; } }
    r
}

fn fe_sqrt(a: &[u64; 4]) -> [u64; 4] {
    let mut r: [u64; 4] = [1, 0, 0, 0];
    let exp = SQRT_EXP;
    for w in 0..4 { let mut bits = exp[3-w]; for _ in 0..64 { r = fe_sqr(&r); if bits >> 63 != 0 { r = fe_mul(&r, a); } bits <<= 1; } }
    r
}

fn fe_to_be_bytes(a: &[u64; 4]) -> [u8; 32] {
    let b0 = a[0].to_le_bytes(); let b1 = a[1].to_le_bytes(); let b2 = a[2].to_le_bytes(); let b3 = a[3].to_le_bytes();
    let le: [u8; 32] = [b0[0],b0[1],b0[2],b0[3],b0[4],b0[5],b0[6],b0[7],
        b1[0],b1[1],b1[2],b1[3],b1[4],b1[5],b1[6],b1[7],
        b2[0],b2[1],b2[2],b2[3],b2[4],b2[5],b2[6],b2[7],
        b3[0],b3[1],b3[2],b3[3],b3[4],b3[5],b3[6],b3[7]];
    let mut be = [0u8; 32]; for i in 0..32 { be[i] = le[31-i]; } be
}

fn be_bytes_to_fe(b: &[u8; 32]) -> [u64; 4] {
    let mut rb = [0u8; 32]; for i in 0..32 { rb[i] = b[31-i]; }
    [u64::from_le_bytes([rb[0],rb[1],rb[2],rb[3],rb[4],rb[5],rb[6],rb[7]]),
     u64::from_le_bytes([rb[8],rb[9],rb[10],rb[11],rb[12],rb[13],rb[14],rb[15]]),
     u64::from_le_bytes([rb[16],rb[17],rb[18],rb[19],rb[20],rb[21],rb[22],rb[23]]),
     u64::from_le_bytes([rb[24],rb[25],rb[26],rb[27],rb[28],rb[29],rb[30],rb[31]])]
}

fn fe_lt(a: &[u64; 4], b: &[u64; 4]) -> bool {
    for i in (0..4).rev() {
        if a[i] < b[i] { return true; }
        if a[i] > b[i] { return false; }
    }
    false
}

// --- Jacobian point arithmetic ---

struct Jac { x: [u64; 4], y: [u64; 4], z: [u64; 4] }

impl Jac {
    fn infinity() -> Self { Jac { x: [0; 4], y: [1; 4], z: [0; 4] } }
    fn is_inf(&self) -> bool { self.z[0] == 0 && self.z[1] == 0 && self.z[2] == 0 && self.z[3] == 0 }
    fn from_affine(x: [u64; 4], y: [u64; 4]) -> Self { Jac { x, y, z: FE_ONE } }
    fn to_affine(&self) -> Option<([u64; 4], [u64; 4])> {
        if self.is_inf() { return None; }
        let z_inv = fe_inv(&self.z);
        let z_inv2 = fe_sqr(&z_inv);
        let z_inv3 = fe_mul(&z_inv2, &z_inv);
        Some((fe_mul(&self.x, &z_inv2), fe_mul(&self.y, &z_inv3)))
    }
}

fn jac_double(p: &Jac) -> Jac {
    if p.is_inf() { return Jac::infinity(); }
    let a = fe_sqr(&p.y);
    let b = fe_mul(&fe_mul(&[4, 0, 0, 0], &p.x), &a);
    let c = fe_sqr(&a);
    let c8 = fe_mul(&[8, 0, 0, 0], &c);
    let xx = fe_sqr(&p.x);
    let d = fe_add(&xx, &fe_add(&xx, &xx)); // 3*X^2
    let x3 = fe_sub(&fe_sqr(&d), &fe_add(&b, &b));
    let y3 = fe_sub(&fe_mul(&d, &fe_sub(&b, &x3)), &c8);
    let z3 = fe_add(&p.y, &p.y); // 2Y
    Jac { x: x3, y: y3, z: fe_mul(&z3, &p.z) }
}

fn jac_add(p: &Jac, q: &Jac) -> Jac {
    if p.is_inf() { return Jac { x: q.x, y: q.y, z: q.z }; }
    if q.is_inf() { return Jac { x: p.x, y: p.y, z: p.z }; }
    let z1z1 = fe_sqr(&p.z);
    let z2z2 = fe_sqr(&q.z);
    let u1 = fe_mul(&p.x, &z2z2);
    let u2 = fe_mul(&q.x, &z1z1);
    let s1 = fe_mul(&p.y, &fe_mul(&q.z, &z2z2));
    let s2 = fe_mul(&q.y, &fe_mul(&p.z, &z1z1));
    let h = fe_sub(&u2, &u1);
    let r = fe_sub(&s2, &s1);
    if (h[0]|h[1]|h[2]|h[3]) == 0 && (r[0]|r[1]|r[2]|r[3]) == 0 { return jac_double(p); }
    if (h[0]|h[1]|h[2]|h[3]) == 0 { return Jac::infinity(); }
    let h2 = fe_sqr(&h);
    let h3 = fe_mul(&h, &h2);
    let u1h2 = fe_mul(&u1, &h2);
    let x3 = fe_sub(&fe_sub(&fe_sqr(&r), &h3), &fe_add(&u1h2, &u1h2));
    let y3 = fe_sub(&fe_mul(&r, &fe_sub(&u1h2, &x3)), &fe_mul(&s1, &h3));
    let z3 = fe_mul(&h, &fe_mul(&p.z, &q.z));
    Jac { x: x3, y: y3, z: z3 }
}

fn scalar_mul(k: &[u64; 4], base: &Jac) -> Jac {
    let mut r = Jac::infinity();
    let mut p = Jac { x: base.x, y: base.y, z: base.z };
    for w in 0..4 { let mut bits = k[w]; for _ in 0..64 { if bits & 1 != 0 { r = jac_add(&r, &p); } p = jac_double(&p); bits >>= 1; } }
    r
}

fn tagged_hash(tag: &str, msg: &[u8]) -> [u8; 32] {
    let tag_hash = Sha256::digest(tag.as_bytes());
    let mut h = Sha256::new();
    h.update(tag_hash); h.update(tag_hash); h.update(msg);
    let r = h.finalize();
    let mut out = [0u8; 32]; out.copy_from_slice(&r); out
}

pub fn schnorr_verify(pk_bytes: &[u8; 32], sig_bytes: &[u8; 64], msg_hash: &[u8; 32]) -> bool {
    let r_bytes: [u8; 32] = sig_bytes[0..32].try_into().unwrap();
    let s_bytes: [u8; 32] = sig_bytes[32..64].try_into().unwrap();
    let r_fe = be_bytes_to_fe(&r_bytes);
    let s_fe = be_bytes_to_fe(&s_bytes);
    let pk_x = be_bytes_to_fe(pk_bytes);
    if !fe_lt(&r_fe, &P) { return false; }
    if !fe_lt(&s_fe, &N) { return false; }
    let x3 = fe_mul(&fe_mul(&pk_x, &pk_x), &pk_x);
    let y_sq = fe_add(&x3, &[7, 0, 0, 0]);
    let y = fe_sqrt(&y_sq);
    if fe_sqr(&y) != y_sq { return false; }
    let y_bytes = fe_to_be_bytes(&y);
    let pk_y = if (y_bytes[31] & 1) != 0 { fe_sub(&P, &y) } else { y };
    let pk = Jac::from_affine(pk_x, pk_y);
    let g = Jac::from_affine(GX, GY);
    let mut buf = [0u8; 96];
    buf[0..32].copy_from_slice(&r_bytes);
    buf[32..64].copy_from_slice(pk_bytes);
    buf[64..96].copy_from_slice(msg_hash);
    let e_fe = be_bytes_to_fe(&tagged_hash("BIP0340/challenge", &buf));
    let e_fe = if !fe_lt(&e_fe, &N) { fe_sub(&e_fe, &N) } else { e_fe };
    let mut neg_e = [0u64; 4];
    let mut borrow: i128 = 0;
    for i in 0..4 { borrow += N[i] as i128 - e_fe[i] as i128; neg_e[i] = borrow as u64; borrow >>= 64; }
    let sg = scalar_mul(&s_fe, &g);
    let neg_ep = scalar_mul(&neg_e, &pk);
    let r_point = jac_add(&sg, &neg_ep);
    let Some((rx, ry)) = r_point.to_affine() else { return false; };
    let r_bytes_computed = fe_to_be_bytes(&rx);
    r_bytes_computed == r_bytes && (fe_to_be_bytes(&ry)[31] & 1) == 0
}
