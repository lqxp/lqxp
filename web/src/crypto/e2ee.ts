const ROOM_ID_BYTES = 16;
const ROOM_KEY_BYTES = 16;
const IV_BYTES = 12;
const TEXT_ENCODER = new TextEncoder();
const TEXT_DECODER = new TextDecoder();

export const E2EE_ENVELOPE_VERSION = 1;
export const E2EE_ALGORITHM = "A128GCM";

function bytesToHex(bytes: Uint8Array) {
  return Array.from(bytes, (value) => value.toString(16).padStart(2, "0")).join("");
}

function hexToBytes(value: string) {
  const normalized = String(value || "").trim().toLowerCase();
  if (!/^[0-9a-f]+$/.test(normalized) || normalized.length % 2 !== 0) {
    throw new Error("Invalid hex payload.");
  }
  const bytes = new Uint8Array(normalized.length / 2);
  for (let index = 0; index < normalized.length; index += 2) {
    bytes[index / 2] = Number.parseInt(normalized.slice(index, index + 2), 16);
  }
  return bytes;
}

function bytesToBase64(bytes: Uint8Array) {
  let binary = "";
  const chunkSize = 0x8000;
  for (let index = 0; index < bytes.length; index += chunkSize) {
    const chunk = bytes.subarray(index, index + chunkSize);
    binary += String.fromCharCode(...chunk);
  }
  return btoa(binary);
}

function base64ToBytes(base64: string) {
  const binary = atob(base64);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index += 1) {
    bytes[index] = binary.charCodeAt(index);
  }
  return bytes;
}

export function encodeBase64Url(bytes: Uint8Array) {
  return bytesToBase64(bytes).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/g, "");
}

export function decodeBase64Url(value: string) {
  const normalized = String(value || "").replace(/-/g, "+").replace(/_/g, "/");
  const padded = normalized + "=".repeat((4 - (normalized.length % 4 || 4)) % 4);
  return base64ToBytes(padded);
}

export function cryptoAvailable() {
  return Boolean(globalThis.crypto?.subtle && globalThis.crypto?.getRandomValues);
}

export function normalizeRoomKey(rawValue: string) {
  const bytes = hexToBytes(String(rawValue || "").trim());
  if (bytes.length !== ROOM_KEY_BYTES) {
    throw new Error("Invalid room key.");
  }
  return bytesToHex(bytes);
}

export function generateRoomKey() {
  if (!cryptoAvailable()) throw new Error("Web Crypto is unavailable.");
  const bytes = new Uint8Array(ROOM_KEY_BYTES);
  globalThis.crypto.getRandomValues(bytes);
  return bytesToHex(bytes);
}

export function normalizeRoomAccessToken(rawValue: string) {
  const normalized = String(rawValue || "").trim().toLowerCase();
  if (!/^[0-9a-f]{64}$/.test(normalized)) {
    throw new Error("Invalid room token.");
  }
  return normalized;
}

export function generateRoomAccessToken() {
  if (!cryptoAvailable()) throw new Error("Web Crypto is unavailable.");
  const roomIdBytes = new Uint8Array(ROOM_ID_BYTES);
  const roomKeyBytes = new Uint8Array(ROOM_KEY_BYTES);
  globalThis.crypto.getRandomValues(roomIdBytes);
  globalThis.crypto.getRandomValues(roomKeyBytes);
  const roomId = bytesToHex(roomIdBytes);
  const roomKey = bytesToHex(roomKeyBytes);
  return {
    roomId,
    roomKey,
    token: `${roomId}${roomKey}`
  };
}

export function parseRoomAccessToken(rawValue: string) {
  const token = normalizeRoomAccessToken(rawValue);
  return {
    token,
    roomId: token.slice(0, ROOM_ID_BYTES * 2),
    roomKey: token.slice(ROOM_ID_BYTES * 2)
  };
}

async function importRoomKey(roomKey: string) {
  if (!cryptoAvailable()) throw new Error("Web Crypto is unavailable.");
  const raw = hexToBytes(roomKey);
  if (raw.length !== ROOM_KEY_BYTES) throw new Error("Invalid room key.");
  return globalThis.crypto.subtle.importKey("raw", raw, { name: "AES-GCM" }, false, ["encrypt", "decrypt"]);
}

export function isEncryptedEnvelope(value: any) {
  return Boolean(
    value
    && typeof value === "object"
    && Number(value.v) === E2EE_ENVELOPE_VERSION
    && String(value.alg || "") === E2EE_ALGORITHM
    && typeof value.iv === "string"
    && typeof value.ciphertext === "string"
  );
}

export async function encryptRoomPayload(roomKey: string, roomId: string, payload: unknown) {
  const key = await importRoomKey(roomKey);
  const iv = new Uint8Array(IV_BYTES);
  globalThis.crypto.getRandomValues(iv);
  const plaintext = TEXT_ENCODER.encode(JSON.stringify(payload));
  const aad = TEXT_ENCODER.encode(String(roomId || ""));
  const ciphertext = await globalThis.crypto.subtle.encrypt(
    { name: "AES-GCM", iv, additionalData: aad },
    key,
    plaintext
  );
  return {
    v: E2EE_ENVELOPE_VERSION,
    alg: E2EE_ALGORITHM,
    iv: encodeBase64Url(iv),
    ciphertext: encodeBase64Url(new Uint8Array(ciphertext))
  };
}

export async function decryptRoomPayload(roomKey: string, roomId: string, envelope: any) {
  if (!isEncryptedEnvelope(envelope)) throw new Error("Invalid encrypted payload.");
  const key = await importRoomKey(roomKey);
  const iv = decodeBase64Url(envelope.iv);
  const ciphertext = decodeBase64Url(envelope.ciphertext);
  const aad = TEXT_ENCODER.encode(String(roomId || ""));
  const plaintext = await globalThis.crypto.subtle.decrypt(
    { name: "AES-GCM", iv, additionalData: aad },
    key,
    ciphertext
  );
  return JSON.parse(TEXT_DECODER.decode(plaintext));
}
