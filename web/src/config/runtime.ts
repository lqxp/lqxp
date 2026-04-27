function normalizedStringArray(value: unknown) {
  return Array.isArray(value)
    ? value.map((item) => String(item || "").trim()).filter(Boolean)
    : [];
}

const rawRtc = window.__QXP_RUNTIME__?.rtc || {};

export const rtcRuntimeConfig = {
  relayOnly: rawRtc.relayOnly !== false,
  turnUrls: normalizedStringArray(rawRtc.turnUrls),
  turnUsername: String(rawRtc.turnUsername || "").trim(),
  turnCredential: String(rawRtc.turnCredential || "").trim(),
  callsEnabled: Boolean(rawRtc.callsEnabled),
  callsUnavailableReason: String(rawRtc.callsUnavailableReason || "").trim()
};
