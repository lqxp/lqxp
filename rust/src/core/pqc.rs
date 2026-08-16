use std::{
    collections::HashMap,
    sync::{Arc, OnceLock},
};

use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

use crate::core::{
    models::now_ms,
    result::{ApiError, ApiResult},
};

pub const PQC_N: usize = 256;
pub const PQC_Q: i32 = 3329;
pub const PQC_HALF_Q: i32 = 1665;
const PQC_KEY_TTL_MS: u64 = 3 * 60 * 1000;

#[derive(Clone, Debug)]
pub struct Poly {
    pub coeffs: [i32; PQC_N],
}

impl Poly {
    pub fn add(&self, other: &Self) -> Self {
        let mut res = [0; PQC_N];
        for ((item, a), b) in res.iter_mut().zip(&self.coeffs).zip(&other.coeffs) {
            *item = (a + b).rem_euclid(PQC_Q);
        }
        Self { coeffs: res }
    }

    pub fn sub(&self, other: &Self) -> Self {
        let mut res = [0; PQC_N];
        for ((item, a), b) in res.iter_mut().zip(&self.coeffs).zip(&other.coeffs) {
            *item = (a - b).rem_euclid(PQC_Q);
        }
        Self { coeffs: res }
    }

    pub fn mul(&self, other: &Self) -> Self {
        let mut res = [0i64; PQC_N];
        for i in 0..PQC_N {
            for j in 0..PQC_N {
                let term = (self.coeffs[i] as i64) * (other.coeffs[j] as i64);
                if i + j < PQC_N {
                    res[i + j] += term;
                } else {
                    res[i + j - PQC_N] -= term;
                }
            }
        }
        let mut out = [0; PQC_N];
        for (out_item, in_item) in out.iter_mut().zip(&res) {
            *out_item = (in_item.rem_euclid(PQC_Q as i64)) as i32;
        }
        Self { coeffs: out }
    }
}

pub fn sample_uniform_poly(seed: &[u8; 32], domain: u8) -> Poly {
    let mut hasher = Sha256::new();
    hasher.update(b"qxprotocol_pqc_uniform_poly:");
    hasher.update(seed);
    hasher.update([domain]);
    let mut digest = hasher.finalize();

    let mut coeffs = [0; PQC_N];
    let mut idx = 0;
    let mut counter = 0u32;

    while idx < PQC_N {
        for chunk in digest.chunks_exact(2) {
            if idx >= PQC_N {
                break;
            }
            let val = u16::from_le_bytes([chunk[0], chunk[1]]) as i32;
            let val = val & 0x0FFF;
            if val < PQC_Q {
                coeffs[idx] = val;
                idx += 1;
            }
        }
        counter += 1;
        let mut h = Sha256::new();
        h.update(b"qxprotocol_pqc_uniform_poly:");
        h.update(seed);
        h.update([domain]);
        h.update(counter.to_le_bytes());
        digest = h.finalize();
    }

    Poly { coeffs }
}

pub fn sample_cbd2_poly(seed: &[u8; 32], domain: u8) -> Poly {
    let mut hasher = Sha256::new();
    hasher.update(b"qxprotocol_pqc_cbd2_poly:");
    hasher.update(seed);
    hasher.update([domain]);
    let mut digest = hasher.finalize();

    let mut coeffs = [0; PQC_N];
    let mut idx = 0;
    let mut counter = 0u32;

    while idx < PQC_N {
        for &byte in digest.iter() {
            if idx >= PQC_N {
                break;
            }
            let a = ((byte & 0x01) + ((byte >> 1) & 0x01)) as i32;
            let b = (((byte >> 2) & 0x01) + ((byte >> 3) & 0x01)) as i32;
            coeffs[idx] = (a - b).rem_euclid(PQC_Q);
            idx += 1;
            if idx >= PQC_N {
                break;
            }
            let a2 = (((byte >> 4) & 0x01) + ((byte >> 5) & 0x01)) as i32;
            let b2 = (((byte >> 6) & 0x01) + ((byte >> 7) & 0x01)) as i32;
            coeffs[idx] = (a2 - b2).rem_euclid(PQC_Q);
            idx += 1;
        }
        counter += 1;
        let mut h = Sha256::new();
        h.update(b"qxprotocol_pqc_cbd2_poly:");
        h.update(seed);
        h.update([domain]);
        h.update(counter.to_le_bytes());
        digest = h.finalize();
    }

    Poly { coeffs }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PqcPublicKey {
    pub key_id: String,
    pub t_hex: String,
    pub rho_hex: String,
}

#[derive(Clone, Debug)]
pub struct PqcSecretKey {
    pub s: Poly,
    pub expires_at: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PqcCiphertext {
    pub key_id: String,
    pub u_hex: String,
    pub v_hex: String,
}

pub fn poly_to_hex(p: &Poly) -> String {
    let mut hex = String::with_capacity(PQC_N * 3);
    for &c in &p.coeffs {
        hex.push_str(&format!("{:03x}", c));
    }
    hex
}

pub fn hex_to_poly(hex_str: &str) -> Option<Poly> {
    if hex_str.len() != PQC_N * 3 {
        return None;
    }
    let mut coeffs = [0; PQC_N];
    for i in 0..PQC_N {
        let chunk = &hex_str[i * 3..(i + 1) * 3];
        let val = i32::from_str_radix(chunk, 16).ok()?;
        if val >= PQC_Q {
            return None;
        }
        coeffs[i] = val;
    }
    Some(Poly { coeffs })
}

pub fn generate_pqc_keypair() -> (PqcPublicKey, PqcSecretKey) {
    let mut key_id_bytes = [0u8; 16];
    let mut rho = [0u8; 32];
    let mut sigma = [0u8; 32];
    OsRng.fill_bytes(&mut key_id_bytes);
    OsRng.fill_bytes(&mut rho);
    OsRng.fill_bytes(&mut sigma);

    let key_id = format!("{:x}", u128::from_le_bytes(key_id_bytes));
    let a = sample_uniform_poly(&rho, 0);
    let s = sample_cbd2_poly(&sigma, 0);
    let e = sample_cbd2_poly(&sigma, 1);

    // t = A * s + e (mod X^256+1, Q)
    let a_mul_s = a.mul(&s);
    let t = a_mul_s.add(&e);

    let now = now_ms();
    let pk = PqcPublicKey {
        key_id,
        t_hex: poly_to_hex(&t),
        rho_hex: format!("{:x}", sha2::Sha256::digest(rho)),
    };
    let sk = PqcSecretKey {
        s,
        expires_at: now + PQC_KEY_TTL_MS,
    };
    (pk, sk)
}

pub fn decapsulate_pqc_secret(
    sk: &PqcSecretKey,
    ct: &PqcCiphertext,
) -> ApiResult<[u8; 32]> {
    let u = hex_to_poly(&ct.u_hex)
        .ok_or_else(|| ApiError::bad_request("Malformed PQC ciphertext component u."))?;
    let v = hex_to_poly(&ct.v_hex)
        .ok_or_else(|| ApiError::bad_request("Malformed PQC ciphertext component v."))?;

    // w = v - s * u
    let s_mul_u = sk.s.mul(&u);
    let w = v.sub(&s_mul_u);

    // Decode 256 message bits
    let mut msg_bytes = [0u8; 32];
    for i in 0..PQC_N {
        let val = w.coeffs[i];
        let diff = (val - PQC_HALF_Q).abs();
        let bit = if diff < (PQC_Q / 4) { 1u8 } else { 0u8 };
        let byte_idx = i / 8;
        let bit_idx = i % 8;
        msg_bytes[byte_idx] |= bit << bit_idx;
    }

    // Derive quantum-safe shared secret: SHA256(msg || u || v)
    let mut hasher = Sha256::new();
    hasher.update(b"qxprotocol_pqc_kem_shared_secret:");
    hasher.update(msg_bytes);
    hasher.update(ct.u_hex.as_bytes());
    hasher.update(ct.v_hex.as_bytes());
    let ss: [u8; 32] = hasher.finalize().into();
    Ok(ss)
}

static PQC_ACTIVE_KEYS: OnceLock<Arc<Mutex<HashMap<String, PqcSecretKey>>>> = OnceLock::new();

fn get_pqc_store() -> &'static Arc<Mutex<HashMap<String, PqcSecretKey>>> {
    PQC_ACTIVE_KEYS.get_or_init(|| Arc::new(Mutex::new(HashMap::new())))
}

pub async fn issue_pqc_challenge() -> PqcPublicKey {
    let (pk, sk) = generate_pqc_keypair();
    let store = get_pqc_store();
    let mut keys = store.lock().await;

    let now = now_ms();
    if keys.len() > 10_000 {
        keys.retain(|_, k| k.expires_at > now);
    }
    keys.insert(pk.key_id.clone(), sk);
    pk
}

pub async fn verify_and_decapsulate_pqc(ct: &PqcCiphertext) -> ApiResult<[u8; 32]> {
    let store = get_pqc_store();
    let mut keys = store.lock().await;

    let now = now_ms();
    let sk = keys.remove(&ct.key_id).ok_or_else(|| {
        ApiError::bad_request("PQC ephemeral quantum challenge expired or already consumed.")
    })?;

    if sk.expires_at < now {
        return Err(ApiError::bad_request("PQC ephemeral quantum challenge expired."));
    }

    decapsulate_pqc_secret(&sk, ct)
}
