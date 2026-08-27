use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde_json::Value;

use crate::core::{
    models::PrekeyBundle,
    result::{ApiError, ApiResult},
};

// ── Canonicalisation ─────────────────────────────────────────────────────────
//
// `canonical()` = JSON trié récursivement par clé, séparateurs compacts
// (sans espaces), tableaux dans l'ordre. C'est la forme EXACTE signée par le
// client (le même contrat doit être réimplémenté en TS dans `crypto/phantom.ts`).

fn canonical_json(value: &Value) -> String {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let mut parts = Vec::with_capacity(keys.len());
            for key in keys {
                let val = map.get(key).expect("key present in map");
                parts.push(format!(
                    "{}:{}",
                    serde_json::to_string(key).expect("string key serializes"),
                    canonical_json(val)
                ));
            }
            format!("{{{}}}", parts.join(","))
        }
        Value::Array(items) => {
            let parts: Vec<String> = items.iter().map(canonical_json).collect();
            format!("[{}]", parts.join(","))
        }
        other => serde_json::to_string(other).expect("scalar serializes"),
    }
}

fn decode_hex(value: &str) -> Option<Vec<u8>> {
    if value.len() % 2 != 0 {
        return None;
    }
    (0..value.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&value[i..i + 2], 16).ok())
        .collect()
}

fn decode_b64url(value: &str) -> Option<Vec<u8>> {
    URL_SAFE_NO_PAD.decode(value.trim_end_matches('=')).ok()
}

/// Octets canoniques du bundle SANS les signatures (`sigEcdsa`, `sigMldsa`).
pub fn canonical_prekey_bundle_bytes(bundle: &PrekeyBundle) -> ApiResult<Vec<u8>> {
    let mut value = serde_json::to_value(bundle)
        .map_err(|err| ApiError::internal("Prekey bundle encode", err))?;
    if let Value::Object(map) = &mut value {
        map.remove("sigEcdsa");
        map.remove("sigMldsa");
    }
    Ok(canonical_json(&value).into_bytes())
}

// ── Vérification ECDSA P-256 (signature brute `r‖s`, 64 octets) ──────────────

fn jwk_to_verifying_key(jwk: &Value) -> ApiResult<p256::ecdsa::VerifyingKey> {
    let kty = jwk.get("kty").and_then(Value::as_str).unwrap_or("");
    let crv = jwk.get("crv").and_then(Value::as_str).unwrap_or("");
    if kty != "EC" || crv != "P-256" {
        return Err(ApiError::bad_request("Unsupported device signing key."));
    }
    let x = jwk
        .get("x")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::bad_request("Missing JWK x coordinate."))?;
    let y = jwk
        .get("y")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::bad_request("Missing JWK y coordinate."))?;
    let x_bytes = decode_b64url(x).ok_or_else(|| ApiError::bad_request("Invalid JWK x encoding."))?;
    let y_bytes = decode_b64url(y).ok_or_else(|| ApiError::bad_request("Invalid JWK y encoding."))?;
    if x_bytes.len() != 32 || y_bytes.len() != 32 {
        return Err(ApiError::bad_request("Invalid JWK coordinate length."));
    }

    let mut encoded = Vec::with_capacity(65);
    encoded.push(0x04);
    encoded.extend_from_slice(&x_bytes);
    encoded.extend_from_slice(&y_bytes);

    let public_key = p256::PublicKey::from_sec1_bytes(&encoded)
        .map_err(|_| ApiError::bad_request("Invalid P-256 public key."))?;
    Ok(p256::ecdsa::VerifyingKey::from(public_key))
}

pub fn verify_ecdsa_p256(jwk: &Value, msg: &[u8], sig_b64url: &str) -> ApiResult<()> {
    use p256::ecdsa::signature::Verifier as _;

    let verifying_key = jwk_to_verifying_key(jwk)?;
    let raw = decode_b64url(sig_b64url)
        .ok_or_else(|| ApiError::bad_request("Invalid ECDSA signature encoding."))?;
    if raw.len() != 64 {
        return Err(ApiError::bad_request("Invalid ECDSA signature length."));
    }

    // Web Crypto émet un `r‖s` brut (IEEE P1363), pas du DER.
    let r = p256::FieldBytes::clone_from_slice(&raw[..32]);
    let s = p256::FieldBytes::clone_from_slice(&raw[32..]);
    let signature = p256::ecdsa::Signature::from_scalars(r, s)
        .map_err(|_| ApiError::bad_request("Invalid ECDSA signature scalars."))?;

    verifying_key
        .verify(msg, &signature)
        .map_err(|_| ApiError::bad_request("Invalid ECDSA signature."))
}

// ── Vérification ML-DSA-65 (FIPS 204) ────────────────────────────────────────

pub fn verify_mldsa65(pk_hex: &str, msg: &[u8], sig_hex: &str) -> ApiResult<()> {
    use ml_dsa::signature::Verifier as _;
    use ml_dsa::{MlDsa65, Signature, VerifyingKey};

    let pk_bytes = decode_hex(pk_hex)
        .ok_or_else(|| ApiError::bad_request("Invalid ML-DSA public key encoding."))?;
    let sig_bytes = decode_hex(sig_hex)
        .ok_or_else(|| ApiError::bad_request("Invalid ML-DSA signature encoding."))?;

    let encoded_vk = ml_dsa::EncodedVerifyingKey::<MlDsa65>::try_from(pk_bytes.as_slice())
        .map_err(|_| ApiError::bad_request("Invalid ML-DSA-65 public key length."))?;
    let verification_key = VerifyingKey::<MlDsa65>::decode(&encoded_vk);
    let signature = Signature::<MlDsa65>::try_from(sig_bytes.as_slice())
        .map_err(|_| ApiError::bad_request("Invalid ML-DSA-65 signature."))?;

    verification_key
        .verify(msg, &signature)
        .map_err(|_| ApiError::bad_request("Invalid ML-DSA-65 signature."))
}

/// Vérifie les DEUX signatures hybrides du bundle (ECDSA P-256 ‖ ML-DSA-65).
pub fn verify_prekey_bundle(bundle: &PrekeyBundle) -> ApiResult<()> {
    let msg = canonical_prekey_bundle_bytes(bundle)?;
    verify_ecdsa_p256(&bundle.ecdsa_p256_pk, &msg, &bundle.sig_ecdsa)?;
    verify_mldsa65(&bundle.mldsa65_pk, &msg, &bundle.sig_mldsa)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn canonical_json_sorts_keys_recursively() {
        let value = json!({ "b": 2, "a": { "d": 1, "c": [3, 1, 2] } });
        assert_eq!(
            canonical_json(&value),
            r#"{"a":{"c":[3,1,2],"d":1},"b":2}"#
        );
    }

    #[test]
    fn canonical_bundle_excludes_signatures() {
        let bundle = PrekeyBundle {
            version: 1,
            mlkem768_pk: "aa".repeat(1184),
            ecdsa_p256_pk: json!({ "kty": "EC", "crv": "P-256", "x": "AQID", "y": "BAUG" }),
            mldsa65_pk: "bb".repeat(1952),
            sig_ecdsa: "c2ln".to_string(),
            sig_mldsa: "cc".repeat(3309),
            block_filter: vec![],
            updated_at: 1730000000000,
        };
        let bytes = canonical_prekey_bundle_bytes(&bundle).expect("canonical");
        let text = String::from_utf8(bytes).expect("utf8");
        assert!(!text.contains("sigEcdsa"));
        assert!(!text.contains("sigMldsa"));
        assert!(text.contains("mlkem768Pk"));
    }

    #[test]
    fn canonical_bundle_matches_client_contract() {
        // Vecteur cross-langage : la sortie doit être identique à celle de
        // `web/src/crypto/phantom.selfcheck.ts` (CROSS_LANG_CANONICAL).
        let bundle = PrekeyBundle {
            version: 1,
            mlkem768_pk: "aa".to_string(),
            ecdsa_p256_pk: json!({ "kty": "EC", "crv": "P-256", "x": "AQID", "y": "BAUG" }),
            mldsa65_pk: "bb".to_string(),
            sig_ecdsa: "c2ln".to_string(),
            sig_mldsa: "cc".to_string(),
            block_filter: vec![],
            updated_at: 1730000000000,
        };
        let bytes = canonical_prekey_bundle_bytes(&bundle).expect("canonical");
        let text = String::from_utf8(bytes).expect("utf8");
        assert_eq!(
            text,
            r#"{"blockFilter":[],"ecdsaP256Pk":{"crv":"P-256","kty":"EC","x":"AQID","y":"BAUG"},"mldsa65Pk":"bb","mlkem768Pk":"aa","updatedAt":1730000000000,"version":1}"#
        );
    }

    #[test]
    fn ecdsa_p256_roundtrip_raw_signature() {
        use p256::ecdsa::signature::Signer as _;
        use p256::elliptic_curve::PrimeField as _;

        let signing_key = p256::ecdsa::SigningKey::random(&mut rand::rngs::OsRng);
        let verifying_key = signing_key.verifying_key();
        let public_key = verifying_key.to_encoded_point(false);
        let x = public_key.as_bytes()[1..33].to_vec();
        let y = public_key.as_bytes()[33..65].to_vec();

        let jwk = json!({
            "kty": "EC",
            "crv": "P-256",
            "x": URL_SAFE_NO_PAD.encode(&x),
            "y": URL_SAFE_NO_PAD.encode(&y),
        });

        let msg = b"canonical prekey bundle bytes";
        let der_sig: p256::ecdsa::Signature = signing_key.sign(msg);
        let (r, s) = der_sig.split_scalars();
        let mut raw = Vec::with_capacity(64);
        raw.extend_from_slice(r.to_repr().as_slice());
        raw.extend_from_slice(s.to_repr().as_slice());
        let sig_b64url = URL_SAFE_NO_PAD.encode(&raw);

        assert!(verify_ecdsa_p256(&jwk, msg, &sig_b64url).is_ok());
    }

    #[test]
    fn mldsa65_roundtrip() {
        use ml_dsa::signature::Signer as _;
        use ml_dsa::{Generate, Keypair, MlDsa65, SigningKey};

        let signing_key = SigningKey::<MlDsa65>::generate();
        let verification_key = signing_key.verifying_key();
        let msg = b"canonical prekey bundle bytes";
        let signature: ml_dsa::Signature<MlDsa65> = signing_key.try_sign(msg).expect("sign");

        let pk_hex = hex_encode(verification_key.encode().as_slice());
        let sig_hex = hex_encode(signature.encode().as_slice());

        assert!(verify_mldsa65(&pk_hex, msg, &sig_hex).is_ok());
    }

    fn hex_encode(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }
}
