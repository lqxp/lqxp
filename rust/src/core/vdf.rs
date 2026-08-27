use std::sync::OnceLock;

use hmac::{Hmac, Mac};
use num_bigint::{BigUint, RandBigInt};
use num_traits::{One, Zero};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::core::{
    models::now_ms,
    result::{ApiError, ApiResult},
};

type HmacSha256 = Hmac<Sha256>;

pub const DEFAULT_VDF_ITERATIONS: u32 = 30_000;
const VDF_TTL_MS: u64 = 3 * 60 * 1000;

// Deterministic 1024-bit RSA Modulus N (Composite of two 512-bit safe primes)
// Generated once with verifiable seed
const VDF_RSA_MODULUS_HEX: &str = "\
c4193b2a8d3e91f07cb658da841793cf445037d0fa049c636fdb3a726ea7b218\
02d33454b5df5b5463690d3d52674e2a9b3425ea3f4581f14ec6e19f7e841249\
91a5db4b4b20a4b3d758f12cb84931a7f0524cb11efaa73d32cb5e09841f39be\
f8da463e26cb3b9a7c36a43e4980a31ebf9a74070a256920f065365e94b281f3";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VdfChallenge {
    pub modulus: String,
    pub x: String,
    pub t: u32,
    pub target_hash: String,
    pub salt: String,
    pub signature: String,
    pub issued_at: u64,
    pub expires_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VdfProof {
    pub y: String,
    pub pi: String,
}

static SERVER_VDF_SECRET: OnceLock<[u8; 32]> = OnceLock::new();
static MODULUS_CACHE: OnceLock<BigUint> = OnceLock::new();

fn get_vdf_secret() -> &'static [u8; 32] {
    SERVER_VDF_SECRET.get_or_init(|| {
        let mut key = [0u8; 32];
        OsRng.fill_bytes(&mut key);
        key
    })
}

pub fn get_modulus() -> &'static BigUint {
    MODULUS_CACHE.get_or_init(|| {
        BigUint::parse_bytes(VDF_RSA_MODULUS_HEX.as_bytes(), 16)
            .expect("Valid hex for VDF modulus")
    })
}

fn sign_challenge(content: &str) -> String {
    let secret = get_vdf_secret();
    let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC can take any key size");
    mac.update(content.as_bytes());
    format!("{:x}", mac.finalize().into_bytes())
}

fn verify_signature(content: &str, signature: &str) -> bool {
    let secret = get_vdf_secret();
    let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC can take any key size");
    mac.update(content.as_bytes());
    let expected = format!("{:x}", mac.finalize().into_bytes());

    if expected.len() != signature.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in expected.bytes().zip(signature.bytes()) {
        diff |= a ^ b;
    }
    diff == 0
}

pub fn hash_target(target: &str) -> String {
    let mut h = Sha256::new();
    h.update(target.trim().to_lowercase().as_bytes());
    format!("{:x}", h.finalize())[..16].to_owned()
}

pub fn hash_to_element(target: &str, salt: &str, modulus: &BigUint) -> BigUint {
    let mut h = Sha256::new();
    h.update(b"qxprotocol_vdf_x_element:");
    h.update(target.trim().to_lowercase().as_bytes());
    h.update(b":");
    h.update(salt.as_bytes());
    let digest = h.finalize();
    let val = BigUint::from_bytes_be(&digest);
    // Ensure 2 <= x < modulus
    (val % (modulus - 3u32)) + 2u32
}

// Deterministic 128-bit prime derived from (x, y) using Fiat-Shamir heuristic
pub fn hash_to_prime(x: &BigUint, y: &BigUint) -> BigUint {
    let mut counter = 0u32;
    loop {
        let mut h = Sha256::new();
        h.update(b"qxprotocol_vdf_prime_l:");
        h.update(x.to_bytes_be());
        h.update(b":");
        h.update(y.to_bytes_be());
        h.update(b":");
        h.update(counter.to_be_bytes());
        let digest = h.finalize();

        // 128-bit odd candidate
        let mut candidate_bytes = [0u8; 16];
        candidate_bytes.copy_from_slice(&digest[..16]);
        candidate_bytes[15] |= 1; // force odd
        candidate_bytes[0] |= 0x80; // force top bit 1

        let prime_candidate = BigUint::from_bytes_be(&candidate_bytes);
        if is_probable_prime(&prime_candidate, 16) {
            return prime_candidate;
        }
        counter += 1;
    }
}

// Miller-Rabin primality test
fn is_probable_prime(n: &BigUint, rounds: usize) -> bool {
    let two = BigUint::from(2u32);
    if n <= &two {
        return n == &two;
    }
    if n % &two == BigUint::zero() {
        return false;
    }

    let one = BigUint::one();
    let n_minus_one = n - &one;
    let mut d = n_minus_one.clone();
    let mut s = 0u32;
    while &d % &two == BigUint::zero() {
        d /= &two;
        s += 1;
    }

    let mut rng = OsRng;
    for _ in 0..rounds {
        let a = rng.gen_biguint_range(&two, &(n - &one));
        let mut x = a.modpow(&d, n);
        if x == one || x == n_minus_one {
            continue;
        }
        let mut composite = true;
        for _ in 0..(s - 1) {
            x = x.modpow(&two, n);
            if x == n_minus_one {
                composite = false;
                break;
            }
        }
        if composite {
            return false;
        }
    }
    true
}

pub fn generate_vdf_challenge(target: Option<&str>, iterations_override: Option<u32>) -> VdfChallenge {
    let now = now_ms();
    let expires_at = now + VDF_TTL_MS;
    let t = iterations_override.unwrap_or(DEFAULT_VDF_ITERATIONS);
    let mut salt_bytes = [0u8; 8];
    OsRng.fill_bytes(&mut salt_bytes);
    let salt = format!("{:x}", u64::from_le_bytes(salt_bytes));

    let target_hash = target
        .map(hash_target)
        .unwrap_or_else(|| "none".to_owned());

    let modulus = get_modulus();
    let x = hash_to_element(target.unwrap_or(""), &salt, modulus);
    let x_hex = format!("{:x}", x);
    let modulus_hex = format!("{:x}", modulus);

    let content_to_sign = format!("{now}:{t}:{target_hash}:{salt}:{x_hex}:{expires_at}");
    let signature = sign_challenge(&content_to_sign);

    VdfChallenge {
        modulus: modulus_hex,
        x: x_hex,
        t,
        target_hash,
        salt,
        signature,
        issued_at: now,
        expires_at,
    }
}

static VDF_CONSUMED: OnceLock<tokio::sync::Mutex<std::collections::HashMap<String, u64>>> = OnceLock::new();

fn get_vdf_consumed() -> &'static tokio::sync::Mutex<std::collections::HashMap<String, u64>> {
    VDF_CONSUMED.get_or_init(|| tokio::sync::Mutex::new(std::collections::HashMap::new()))
}

pub async fn verify_and_consume_vdf(
    challenge: &VdfChallenge,
    proof: &VdfProof,
    expected_target: Option<&str>,
) -> ApiResult<()> {
    verify_vdf(challenge, proof, expected_target)?;

    let now = now_ms();
    let consumed_arc = get_vdf_consumed();
    let mut consumed = consumed_arc.lock().await;

    if consumed.len() > 10_000 {
        consumed.retain(|_, expires| *expires > now);
    }

    if let Some(&expires) = consumed.get(&challenge.signature) {
        if expires > now {
            return Err(ApiError::too_many_requests(
                "Security challenge has already been used. Please request a new one.",
            ));
        }
    }

    consumed.insert(challenge.signature.clone(), challenge.expires_at);
    Ok(())
}

pub fn verify_vdf(
    challenge: &VdfChallenge,
    proof: &VdfProof,
    expected_target: Option<&str>,
) -> ApiResult<()> {
    let now = now_ms();
    if now > challenge.expires_at {
        return Err(ApiError::bad_request("VDF security challenge expired."));
    }

    let content_to_verify = format!(
        "{}:{}:{}:{}:{}:{}",
        challenge.issued_at,
        challenge.t,
        challenge.target_hash,
        challenge.salt,
        challenge.x,
        challenge.expires_at
    );

    if !verify_signature(&content_to_verify, &challenge.signature) {
        return Err(ApiError::bad_request("Invalid VDF security challenge signature."));
    }

    if let Some(target) = expected_target {
        let expected_hash = hash_target(target);
        let left = challenge.target_hash.as_bytes();
        let right = expected_hash.as_bytes();
        // Comparaison constant-time (S4).
        if left.len() != right.len() || !bool::from(left.ct_eq(right)) {
            return Err(ApiError::bad_request(
                "VDF security challenge was not bound to this target username.",
            ));
        }
    }

    // Borne de taille AVANT parse_bytes (S3) : évite l'allocation de grands
    // entiers sur entrée non fiable.
    if challenge.x.len() > 1024 || proof.y.len() > 1024 || proof.pi.len() > 1024 {
        return Err(ApiError::bad_request("VDF parameters exceed size bound."));
    }

    let modulus = get_modulus();
    let x = BigUint::parse_bytes(challenge.x.as_bytes(), 16)
        .ok_or_else(|| ApiError::bad_request("Malformed VDF parameter x."))?;
    let y = BigUint::parse_bytes(proof.y.as_bytes(), 16)
        .ok_or_else(|| ApiError::bad_request("Malformed VDF result y."))?;
    let pi = BigUint::parse_bytes(proof.pi.as_bytes(), 16)
        .ok_or_else(|| ApiError::bad_request("Malformed VDF proof pi."))?;

    let two = BigUint::from(2u32);
    if x < two || y < two || pi < two || &x >= modulus || &y >= modulus || &pi >= modulus {
        return Err(ApiError::bad_request("VDF values exceed modulus bound."));
    }

    // Fiat-Shamir prime l = HashToPrime(x, y)
    let l = hash_to_prime(&x, &y);

    // r = 2^T mod l (in O(log T))
    let two = BigUint::from(2u32);
    let t_bn = BigUint::from(challenge.t);
    let r = two.modpow(&t_bn, &l);

    // (pi^l * x^r) mod N == y mod N
    let pi_l = pi.modpow(&l, modulus);
    let x_r = x.modpow(&r, modulus);
    let check = (pi_l * x_r) % modulus;

    if check != y {
        return Err(ApiError::bad_request("Invalid VDF delay proof."));
    }

    Ok(())
}
