use std::{
    collections::HashMap,
    sync::{Arc, OnceLock},
};

use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use ml_kem::kem::{Decapsulate, Kem, KeyExport};
use ml_kem::MlKem768;

use crate::core::{
    models::now_ms,
    result::{ApiError, ApiResult},
};

const PQC_KEY_TTL_MS: u64 = 3 * 60 * 1000;

/// Clé d'encapsulation ML-KEM-768 (FIPS 203) renvoyée au client pour le
/// challenge anti-bot. `ek_hex` = 1184 octets hexadécimaux.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PqcPublicKey {
    pub key_id: String,
    pub ek_hex: String,
}

/// Ciphertext ML-KEM-768 (FIPS 203) renvoyé par le client. `ct_hex` = 1088
/// octets hexadécimaux.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PqcCiphertext {
    pub key_id: String,
    pub ct_hex: String,
}

struct PqcSecretKey {
    dk: ml_kem::DecapsulationKey<MlKem768>,
    expires_at: u64,
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn hex_decode(value: &str) -> Option<Vec<u8>> {
    if value.len() % 2 != 0 {
        return None;
    }
    (0..value.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&value[i..i + 2], 16).ok())
        .collect()
}

static PQC_ACTIVE_KEYS: OnceLock<Arc<Mutex<HashMap<String, PqcSecretKey>>>> = OnceLock::new();

fn get_pqc_store() -> &'static Arc<Mutex<HashMap<String, PqcSecretKey>>> {
    PQC_ACTIVE_KEYS.get_or_init(|| Arc::new(Mutex::new(HashMap::new())))
}

/// Génère une paire ML-KEM-768 éphémère, stocke la clé de décapsulation en RAM
/// (TTL 3 min) et renvoie la clé d'encapsulation au client.
pub async fn issue_pqc_challenge() -> PqcPublicKey {
    let mut key_id_bytes = [0u8; 16];
    OsRng.fill_bytes(&mut key_id_bytes);
    let key_id = format!("{:x}", u128::from_le_bytes(key_id_bytes));

    let (dk, ek) = MlKem768::generate_keypair();
    let ek_bytes = ek.to_bytes();
    let pk = PqcPublicKey {
        key_id: key_id.clone(),
        ek_hex: hex_encode(ek_bytes.as_slice()),
    };

    let sk = PqcSecretKey {
        dk,
        expires_at: now_ms() + PQC_KEY_TTL_MS,
    };

    let store = get_pqc_store();
    let mut keys = store.lock().await;
    let now = now_ms();
    if keys.len() > 10_000 {
        keys.retain(|_, k| k.expires_at > now);
    }
    keys.insert(key_id, sk);
    pk
}

/// Décapsule le ciphertext ML-KEM-768 et renvoie le secret partagé (32 octets).
/// Consomme la clé éphémère correspondante (one-time).
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

    let ct_bytes = hex_decode(&ct.ct_hex)
        .ok_or_else(|| ApiError::bad_request("Malformed ML-KEM ciphertext encoding."))?;

    let shared = sk
        .dk
        .decapsulate_slice(&ct_bytes)
        .map_err(|_| ApiError::bad_request("ML-KEM decapsulation failed."))?;

    let mut out = [0u8; 32];
    out.copy_from_slice(shared.as_slice());
    Ok(out)
}
