use std::{
    collections::HashMap,
    sync::{Arc, OnceLock},
};

use hmac::{Hmac, Mac};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use tokio::sync::Mutex;

use crate::core::{
    models::now_ms,
    pqc::{issue_pqc_challenge, verify_and_decapsulate_pqc, PqcCiphertext, PqcPublicKey},
    result::{ApiError, ApiResult},
    rln::{generate_quota_token, verify_and_consume_nullifier, EpochQuotaToken},
    vdf::{generate_vdf_challenge, verify_and_consume_vdf, VdfChallenge, VdfProof},
};

type HmacSha256 = Hmac<Sha256>;

const CAP_CHALLENGE_TTL_MS: u64 = 3 * 60 * 1000;
const CAP_TOKEN_TTL_MS: u64 = 5 * 60 * 1000;
const MIN_HUMAN_INTERACTION_MS: u64 = 250;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapChallenge {
    pub challenge_id: String,
    pub scope: String,
    pub issued_at: u64,
    pub expires_at: u64,
    pub vdf: VdfChallenge,
    pub quota_token: EpochQuotaToken,
    pub pqc_key: PqcPublicKey,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapInstrumentation {
    pub interaction_time_ms: Option<u64>,
    pub entropy: Option<f64>,
    pub webdriver: Option<bool>,
    pub devtools_detected: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapRedeemRequest {
    pub challenge_id: String,
    pub scope: String,
    pub challenge: CapChallenge,
    pub vdf_proof: VdfProof,
    pub nullifier: String,
    pub pqc_ciphertext: PqcCiphertext,
    pub instrumentation: Option<CapInstrumentation>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapRedeemResponse {
    pub success: bool,
    pub cap_token: String,
    pub expires_at: u64,
}

static CAP_SECRET: OnceLock<[u8; 32]> = OnceLock::new();
static CONSUMED_CAP_TOKENS: OnceLock<Arc<Mutex<HashMap<String, u64>>>> = OnceLock::new();

fn get_cap_secret() -> &'static [u8; 32] {
    CAP_SECRET.get_or_init(|| {
        let mut key = [0u8; 32];
        OsRng.fill_bytes(&mut key);
        key
    })
}

fn get_token_store() -> &'static Arc<Mutex<HashMap<String, u64>>> {
    CONSUMED_CAP_TOKENS.get_or_init(|| Arc::new(Mutex::new(HashMap::new())))
}

fn sign_data(data: &str) -> String {
    let secret = get_cap_secret();
    let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC key valid");
    mac.update(data.as_bytes());
    format!("{:x}", mac.finalize().into_bytes())
}

fn constant_time_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.bytes().zip(b.bytes()) {
        diff |= x ^ y;
    }
    diff == 0
}

pub async fn generate_cap_challenge(scope: &str, target: Option<&str>) -> CapChallenge {
    let mut rand_bytes = [0u8; 16];
    OsRng.fill_bytes(&mut rand_bytes);
    let challenge_id = format!("{:x}", u128::from_le_bytes(rand_bytes));

    let now = now_ms();
    let expires_at = now + CAP_CHALLENGE_TTL_MS;

    let vdf = generate_vdf_challenge(target, None);
    let quota_token = generate_quota_token();
    let pqc_key = issue_pqc_challenge().await;

    let canonical_data = format!(
        "{}:{}:{}:{}:{}:{}",
        challenge_id, scope, now, expires_at, vdf.signature, pqc_key.key_id
    );
    let signature = sign_data(&canonical_data);

    CapChallenge {
        challenge_id,
        scope: scope.to_owned(),
        issued_at: now,
        expires_at,
        vdf,
        quota_token,
        pqc_key,
        signature,
    }
}

pub async fn redeem_cap_challenge(req: CapRedeemRequest) -> ApiResult<CapRedeemResponse> {
    let now = now_ms();

    if req.challenge_id != req.challenge.challenge_id {
        return Err(ApiError::bad_request("CAPTCHA challenge ID mismatch."));
    }
    if now > req.challenge.expires_at {
        return Err(ApiError::bad_request("CAPTCHA challenge expired."));
    }
    if req.challenge.scope != req.scope {
        return Err(ApiError::bad_request("CAPTCHA challenge scope mismatch."));
    }

    let canonical_data = format!(
        "{}:{}:{}:{}:{}:{}",
        req.challenge.challenge_id,
        req.challenge.scope,
        req.challenge.issued_at,
        req.challenge.expires_at,
        req.challenge.vdf.signature,
        req.challenge.pqc_key.key_id
    );
    let expected_sig = sign_data(&canonical_data);
    if !constant_time_eq(&expected_sig, &req.challenge.signature) {
        return Err(ApiError::bad_request("Invalid CAPTCHA challenge signature."));
    }

    verify_and_consume_nullifier(&req.challenge.quota_token, &req.nullifier, &req.scope).await?;
    verify_and_consume_vdf(&req.challenge.vdf, &req.vdf_proof, None).await?;
    let _shared_secret = verify_and_decapsulate_pqc(&req.pqc_ciphertext).await?;

    if let Some(instr) = &req.instrumentation {
        if instr.webdriver == Some(true) {
            return Err(ApiError::bad_request("Automated browser execution detected."));
        }
        if let Some(time) = instr.interaction_time_ms {
            if time < MIN_HUMAN_INTERACTION_MS {
                return Err(ApiError::bad_request("Interaction time below human threshold."));
            }
        }
    }

    let mut token_entropy = [0u8; 16];
    OsRng.fill_bytes(&mut token_entropy);
    let token_id = format!("{:x}", u128::from_le_bytes(token_entropy));
    let token_expires = now + CAP_TOKEN_TTL_MS;

    let token_data = format!("{}:{}:{}", token_id, req.scope, token_expires);
    let token_sig = sign_data(&token_data);
    let cap_token = format!("cap.{}.{}.{}", token_id, token_expires, token_sig);

    let store = get_token_store();
    {
        let mut map = store.lock().await;
        map.retain(|_, exp| *exp > now);
        map.insert(cap_token.clone(), token_expires);
    }

    Ok(CapRedeemResponse {
        success: true,
        cap_token,
        expires_at: token_expires,
    })
}

pub async fn verify_and_consume_cap_token(token: &str, expected_scope: &str) -> ApiResult<()> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 4 || parts[0] != "cap" {
        return Err(ApiError::bad_request("Invalid CAPTCHA token format."));
    }

    let token_id = parts[1];
    let expires_str = parts[2];
    let sig = parts[3];

    let expires_at = expires_str
        .parse::<u64>()
        .map_err(|_| ApiError::bad_request("Invalid CAPTCHA token timestamp."))?;

    let now = now_ms();
    if now > expires_at {
        return Err(ApiError::bad_request("CAPTCHA token expired. Please solve again."));
    }

    let token_data = format!("{}:{}:{}", token_id, expected_scope, expires_at);
    let expected_sig = sign_data(&token_data);
    if !constant_time_eq(&expected_sig, sig) {
        return Err(ApiError::bad_request("Invalid CAPTCHA token signature."));
    }

    let store = get_token_store();
    let mut map = store.lock().await;
    match map.remove(token) {
        Some(exp) if exp > now => Ok(()),
        _ => Err(ApiError::bad_request("CAPTCHA token has already been consumed or is invalid.")),
    }
}
