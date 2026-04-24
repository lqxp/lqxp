import { computed, nextTick, reactive } from "vue";

const STORAGE_KEY = "qxprotocol-messenger-v4";
const QUICK_REACTIONS = ["❤️", "👍", "😂", "😮", "😢", "🙏"];
const MAX_ROOMS_SHOWN = 100;
const MAX_HISTORY_PER_ROOM = 500;
const ROOM_ID_MIN_LENGTH = 8;
const ROOM_ID_MAX_LENGTH = 64;
const MESSAGE_LIMIT = 2000;
const MAX_ATTACHMENT_BYTES = 10 * 1024 * 1024;
const CALL_CHUNK_MS = 800;
const RANDOM_ROOM_ALPHABET = "abcdefghijklmnopqrstuvwxyz0123456789";

function inferWebSocketUrl() {
  const protocol = window.location.protocol === "https:" ? "wss:" : "ws:";
  return `${protocol}//${window.location.host}/ws`;
}

function sanitizeUsername(value) {
  return String(value || "").trim().slice(0, 16);
}

function sanitizeRoomId(value) {
  return String(value || "").trim().slice(0, ROOM_ID_MAX_LENGTH);
}

function validateRoomId(value) {
  const id = sanitizeRoomId(value);
  if (id.length < ROOM_ID_MIN_LENGTH) return "Room name must be at least 8 characters.";
  if (id.length > ROOM_ID_MAX_LENGTH) return "Room name must be at most 64 characters.";
  if (!/^[a-z0-9]+$/i.test(id)) return "Room name can only contain letters and numbers.";
  return "";
}

function isValidRoomId(value) {
  return !validateRoomId(value);
}

function generateRandomRoomId() {
  const cryptoApi = globalThis.crypto;
  if (!cryptoApi?.getRandomValues) {
    throw new Error("Browser crypto API is unavailable.");
  }

  let id = "";
  const bytes = new Uint8Array(ROOM_ID_MAX_LENGTH);
  const maxByte = Math.floor(256 / RANDOM_ROOM_ALPHABET.length) * RANDOM_ROOM_ALPHABET.length;
  while (id.length < ROOM_ID_MAX_LENGTH) {
    cryptoApi.getRandomValues(bytes);
    for (const byte of bytes) {
      if (byte >= maxByte) continue;
      id += RANDOM_ROOM_ALPHABET[byte % RANDOM_ROOM_ALPHABET.length];
      if (id.length === ROOM_ID_MAX_LENGTH) break;
    }
  }
  return id;
}

async function copyTextToClipboard(text) {
  if (navigator.clipboard?.writeText) {
    await navigator.clipboard.writeText(text);
    return;
  }

  const textarea = document.createElement("textarea");
  textarea.value = text;
  textarea.setAttribute("readonly", "");
  textarea.style.position = "fixed";
  textarea.style.opacity = "0";
  document.body.appendChild(textarea);
  textarea.select();
  const copied = document.execCommand("copy");
  textarea.remove();
  if (!copied) throw new Error("Clipboard copy failed.");
}

function formatTime(timestamp) {
  return new Date(timestamp).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}

function formatDay(timestamp) {
  const date = new Date(timestamp);
  const now = new Date();
  const oneDay = 86_400_000;
  if (date.toDateString() === now.toDateString()) return "Today";
  if (date.toDateString() === new Date(now.getTime() - oneDay).toDateString()) return "Yesterday";
  if (now.getTime() - date.getTime() < 7 * oneDay) return date.toLocaleDateString([], { weekday: "long" });
  return date.toLocaleDateString([], { month: "short", day: "numeric", year: date.getFullYear() === now.getFullYear() ? undefined : "numeric" });
}

function formatSidebarTime(timestamp) {
  if (!timestamp) return "";
  const date = new Date(timestamp);
  const now = new Date();
  const oneDay = 86_400_000;
  if (date.toDateString() === now.toDateString()) return formatTime(timestamp);
  if (now.getTime() - date.getTime() < 7 * oneDay) return date.toLocaleDateString([], { weekday: "short" });
  return date.toLocaleDateString([], { month: "short", day: "numeric" });
}

function createDefaultUsername() {
  const alphabet = "abcdefghjkmnpqrstuvwxyz";
  let suffix = "";
  for (let i = 0; i < 4; i += 1) suffix += alphabet[Math.floor(Math.random() * alphabet.length)];
  return `echo-${suffix}`;
}

function accentFor(seed) {
  const palette = ["blue", "green", "amber", "violet", "olive", "slate", "teal", "rose"];
  const s = String(seed || "");
  let h = 0;
  for (let i = 0; i < s.length; i += 1) h = (h * 31 + s.charCodeAt(i)) >>> 0;
  return palette[h % palette.length];
}

function loadPersisted() {
  try {
    const raw = JSON.parse(localStorage.getItem(STORAGE_KEY) || "{}");
    const rooms = Array.isArray(raw.rooms)
      ? raw.rooms
          .filter((r) => r && typeof r === "object" && typeof r.roomId === "string")
          .slice(0, MAX_ROOMS_SHOWN)
          .map((r) => ({
            roomId: sanitizeRoomId(r.roomId),
            lastPreview: String(r.lastPreview || ""),
            lastTimestamp: Number(r.lastTimestamp) || 0,
            lastSender: String(r.lastSender || "")
          }))
          .filter((r) => isValidRoomId(r.roomId))
      : [];

    const messagesByRoom = {};
    if (raw.messagesByRoom && typeof raw.messagesByRoom === "object") {
      for (const [id, arr] of Object.entries(raw.messagesByRoom)) {
        if (!Array.isArray(arr)) continue;
        const roomId = sanitizeRoomId(id);
        if (!isValidRoomId(roomId)) continue;
        messagesByRoom[roomId] = arr
          .slice(-MAX_HISTORY_PER_ROOM)
          .map((m) => normalizeMessage(m, id));
      }
    }

    const unreadByRoom = {};
    if (raw.unreadByRoom && typeof raw.unreadByRoom === "object") {
      for (const [id, n] of Object.entries(raw.unreadByRoom)) {
        const v = Number(n);
        const roomId = sanitizeRoomId(id);
        if (Number.isFinite(v) && v > 0 && isValidRoomId(roomId)) unreadByRoom[roomId] = v;
      }
    }

    return {
      username: String(raw.username || createDefaultUsername()),
      activeRoom: isValidRoomId(raw.activeRoom) ? sanitizeRoomId(raw.activeRoom) : "",
      rooms,
      messagesByRoom,
      unreadByRoom,
      selectedAudioInputId: String(raw.selectedAudioInputId || ""),
      selectedAudioOutputId: String(raw.selectedAudioOutputId || ""),
      microphoneThreshold: Math.max(0, Math.min(100, Number(raw.microphoneThreshold) || 0)),
      deleteMessagesOnLeave: Boolean(raw.deleteMessagesOnLeave),
      streamerMode: Boolean(raw.streamerMode),
      messageSoundEnabled: Boolean(raw.messageSoundEnabled),
      callUserVolumes: sanitizeCallUserVolumes(raw.callUserVolumes)
    };
  } catch {
    return {
      username: createDefaultUsername(),
      activeRoom: "",
      rooms: [],
      messagesByRoom: {},
      unreadByRoom: {},
      selectedAudioInputId: "",
      selectedAudioOutputId: "",
      microphoneThreshold: 0,
      deleteMessagesOnLeave: false,
      streamerMode: false,
      messageSoundEnabled: false,
      callUserVolumes: {}
    };
  }
}

function sanitizeCallUserVolumes(raw) {
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) return {};
  const next = {};
  for (const [name, value] of Object.entries(raw)) {
    const key = sanitizeUsername(name);
    if (!key) continue;
    next[key] = Math.max(0, Math.min(100, Math.round(Number(value) || 0)));
  }
  return next;
}

function stripAttachmentDataForStorage(arr) {
  return (arr || []).map((m) => {
    if (!m?.attachment) return m;
    const { dataB64: _omit, ...rest } = m.attachment;
    return { ...m, attachment: { ...rest, dataB64: "" } };
  });
}

function savePersisted(state) {
  try {
    const messagesByRoom = {};
    for (const [id, arr] of Object.entries(state.messagesByRoom || {})) {
      messagesByRoom[id] = stripAttachmentDataForStorage(arr.slice(-MAX_HISTORY_PER_ROOM));
    }
    localStorage.setItem(
      STORAGE_KEY,
      JSON.stringify({
        version: 4,
        username: sanitizeUsername(state.username),
        activeRoom: sanitizeRoomId(state.activeRoom),
        rooms: state.rooms,
        messagesByRoom,
        unreadByRoom: state.unreadByRoom,
        selectedAudioInputId: state.selectedAudioInputId,
        selectedAudioOutputId: state.selectedAudioOutputId,
        microphoneThreshold: state.microphoneThreshold,
        deleteMessagesOnLeave: state.deleteMessagesOnLeave,
        streamerMode: state.streamerMode,
        messageSoundEnabled: state.messageSoundEnabled,
        callUserVolumes: sanitizeCallUserVolumes(state.callUserVolumes)
      })
    );
  } catch {
    /* storage full — attachment bytes alone can exceed quota */
  }
}

function parseVoiceLabel(text) {
  const match = /^\[voice:(\d+:\d{2})\]$/i.exec(String(text || "").trim());
  return match ? match[1] : "";
}

function extractUsername(label) {
  const parts = String(label || "Unknown").split(" ");
  return parts[parts.length - 1] || "Unknown";
}

function formatSize(bytes) {
  const n = Number(bytes) || 0;
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
}

const _emojiSegmenter =
  typeof Intl !== "undefined" && typeof Intl.Segmenter === "function"
    ? new Intl.Segmenter(undefined, { granularity: "grapheme" })
    : null;

// Identifies messages that are 1–3 pure emoji graphemes (Discord-style jumbo).
// Falls back to a code-point-based heuristic when Intl.Segmenter is missing.
const EMOJI_CHAR_REGEX = /[\p{Emoji_Presentation}\p{Extended_Pictographic}]/u;
function isOnlyEmoji(text) {
  const trimmed = String(text || "").trim();
  if (!trimmed) return false;

  let graphemes;
  if (_emojiSegmenter) {
    graphemes = [..._emojiSegmenter.segment(trimmed)].map((s) => s.segment);
  } else {
    graphemes = Array.from(trimmed);
  }
  graphemes = graphemes.filter((g) => g.trim().length > 0);
  if (graphemes.length === 0 || graphemes.length > 3) return false;
  return graphemes.every((g) => EMOJI_CHAR_REGEX.test(g));
}

function normalizeMessage(message, fallbackRoomId) {
  const voiceDuration = parseVoiceLabel(message.text);
  const attachment = message.attachment && typeof message.attachment === "object"
    ? {
        filename: String(message.attachment.filename || "file"),
        mimeType: String(message.attachment.mimeType || "application/octet-stream"),
        size: Number(message.attachment.size) || 0,
        dataB64: String(message.attachment.dataB64 || "")
      }
    : null;

  let kind = "text";
  if (message.deleted) kind = "deleted";
  else if (voiceDuration) kind = "voice";
  else if (attachment) {
    if ((attachment.mimeType || "").startsWith("audio/")) kind = "audio";
    else if ((attachment.mimeType || "").startsWith("image/")) kind = "image";
    else kind = "file";
  }

  const rawText = message.text || "";
  const jumboEmoji = !attachment && !voiceDuration && !message.deleted && isOnlyEmoji(rawText);

  const preview = message.preview && typeof message.preview === "object"
    ? {
        url: String(message.preview.url || ""),
        title: String(message.preview.title || "").slice(0, 300),
        description: String(message.preview.description || "").slice(0, 500),
        image: String(message.preview.image || ""),
        siteName: String(message.preview.siteName || "").slice(0, 80)
      }
    : null;

  return {
    messageId: message.messageId,
    roomId: message.roomId || fallbackRoomId || "",
    user: message.user || message.username || "Unknown",
    username: message.username || extractUsername(message.user),
    text: voiceDuration ? "Voice message" : rawText,
    rawText,
    timestamp: message.timestamp || Date.now(),
    system: Boolean(message.system),
    deleted: Boolean(message.deleted),
    reactions: Array.isArray(message.reactions) ? message.reactions : [],
    replyToMessageId: String(message.replyToMessageId || ""),
    attachment,
    preview,
    kind,
    voiceDuration,
    jumboEmoji
  };
}

function blobToBase64(blob) {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onerror = () => reject(new Error("read failed"));
    reader.onload = () => {
      const result = String(reader.result || "");
      const comma = result.indexOf(",");
      resolve(comma >= 0 ? result.slice(comma + 1) : result);
    };
    reader.readAsDataURL(blob);
  });
}

function base64ToBlob(b64, mimeType) {
  const bytes = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
  return new Blob([bytes], { type: mimeType || "application/octet-stream" });
}

function microphoneLevelFromSamples(samples) {
  if (!samples?.length) return 0;
  let peak = 0;
  let sum = 0;
  for (const sample of samples) {
    const centered = Math.abs((sample - 128) / 128);
    peak = Math.max(peak, centered);
    sum += centered * centered;
  }
  const rms = Math.sqrt(sum / samples.length);
  const blended = Math.max(rms * 4.5, peak * 1.8);
  return Math.min(100, Math.round(Math.pow(blended, 0.72) * 100));
}

function smoothLevel(previous, next) {
  const attack = 0.38;
  const release = 0.13;
  const factor = next > previous ? attack : release;
  return previous + (next - previous) * factor;
}

function buildWaveform(seed) {
  return Array.from({ length: 28 }, (_, index) => {
    const offset = ((index * 17) + String(seed).length * 13) % 24;
    return 8 + offset;
  });
}

let singleton;

export function useMessenger() {
  if (singleton) return singleton;

  const persisted = loadPersisted();
  let toastTimer = null;

  const state = reactive({
    ws: null,
    connected: false,
    identified: false,
    uuid: null,
    heartbeatInterval: 3000,
    heartbeatTimer: null,
    manualClose: false,

    username: persisted.username,
    activeRoom: persisted.activeRoom,
    rooms: persisted.rooms,

    joinedRooms: [],
    pendingJoinRooms: [],
    messagesByRoom: persisted.messagesByRoom,
    usersByRoom: {},
    unreadByRoom: persisted.unreadByRoom,

    messageInput: "",
    voiceEnabled: false,
    systemBanner: "",
    lastError: "",
    searchTerm: "",

    composing: false,
    composeInput: "",
    toastMessage: "",

    settingsOpen: false,
    replyingTo: null,

    audioDevices: [],
    selectedAudioInputId: persisted.selectedAudioInputId,
    selectedAudioOutputId: persisted.selectedAudioOutputId,
    microphoneThreshold: persisted.microphoneThreshold,
    deleteMessagesOnLeave: persisted.deleteMessagesOnLeave,
    streamerMode: persisted.streamerMode,
    messageSoundEnabled: persisted.messageSoundEnabled,
    callUserVolumes: persisted.callUserVolumes,
    audioDevicesLoading: false,
    audioDevicesPermission: "unknown",
    micTestActive: false,
    micTestLoading: false,
    micTestLevel: 0,

    recording: null,        // { recorder, stream, startedAt, roomId } while recording voice memo
    recordingElapsed: 0,

    inCall: false,          // currently mid-voice-call
    callRoom: "",           // which room the call is in
    callStream: null,       // MediaStream
    callRecorder: null,     // active MediaRecorder chunk
    callTimer: null,        // setTimeout handle
    callElapsed: 0,         // seconds
    callMuted: false,       // local mic mute (tracks state.callStream tracks.enabled)

    voiceMembersByRoom: {}, // { roomId: [username, ...] } — who is currently in voice
    speakingByRoom: {},     // { roomId: { username: lastChunkTimestamp } } — recent speakers
    callAnalyser: null,
    callAnalyserData: null
  });

  // Non-reactive registry of Blob-URLs keyed by messageId so repeated renders
  // reuse the same URL and we can free them when messages are evicted.
  const attachmentUrlCache = new Map();
  let micTestStream = null;
  let micTestAudio = null;
  let micTestFrame = 0;
  let micTestAnalyser = null;
  let micTestAnalyserData = null;
  let micTestSmoothedLevel = 0;
  let notificationAudioContext = null;

  function attachmentUrlFor(message) {
    if (!message?.attachment?.dataB64) return null;
    const id = message.messageId;
    if (attachmentUrlCache.has(id)) return attachmentUrlCache.get(id);
    try {
      const blob = base64ToBlob(message.attachment.dataB64, message.attachment.mimeType);
      const url = URL.createObjectURL(blob);
      attachmentUrlCache.set(id, url);
      return url;
    } catch {
      return null;
    }
  }

  const roomLabel = computed(() => state.activeRoom);
  const roomTitle = computed(() => state.activeRoom ? displayRoomName(state.activeRoom) : "No conversation");

  const connectionLabel = computed(() => {
    if (state.connected && state.identified) return "online";
    if (state.connected) return "authenticating";
    return "offline";
  });

  const onlineCount = computed(() => (state.usersByRoom[state.activeRoom] || []).length);

  const sortedMessages = computed(() => {
    const arr = state.messagesByRoom[state.activeRoom] || [];
    return [...arr].sort((a, b) => (a.timestamp || 0) - (b.timestamp || 0));
  });

  const canSend = computed(() => state.messageInput.trim().length > 0 && !!state.activeRoom);

  const conversations = computed(() => {
    const query = state.searchTerm.trim().toLowerCase();
    return state.rooms
      .slice()
      .sort((a, b) => (b.lastTimestamp || 0) - (a.lastTimestamp || 0))
      .filter((r) => !query || r.roomId.toLowerCase().includes(query) || r.lastPreview.toLowerCase().includes(query))
      .map((r) => ({
        roomId: r.roomId,
        name: displayRoomName(r.roomId),
        accent: accentFor(r.roomId),
        preview: r.lastPreview || "No messages yet",
        timestampLabel: formatSidebarTime(r.lastTimestamp),
        active: r.roomId === state.activeRoom,
        unread: state.unreadByRoom[r.roomId] || 0,
        joined: state.joinedRooms.includes(r.roomId)
      }));
  });

  const activeConversation = computed(() => {
    if (!state.activeRoom) return null;
    const found = conversations.value.find((c) => c.active);
    if (found) return found;
    return {
      roomId: state.activeRoom,
      name: displayRoomName(state.activeRoom),
      accent: accentFor(state.activeRoom),
      active: true,
      preview: "",
      timestampLabel: "",
      unread: 0,
      joined: state.joinedRooms.includes(state.activeRoom)
    };
  });

  const memberRoster = computed(() => state.usersByRoom[state.activeRoom] || []);

  function persist() {
    savePersisted(state);
  }

  function displayRoomName(roomId) {
    const id = sanitizeRoomId(roomId);
    if (!id) return "";
    return state.streamerMode ? "Hidden channel" : id;
  }

  function setDeleteMessagesOnLeave(value) {
    state.deleteMessagesOnLeave = Boolean(value);
    persist();
  }

  function setStreamerMode(value) {
    state.streamerMode = Boolean(value);
    persist();
  }

  function setMessageSoundEnabled(value) {
    state.messageSoundEnabled = Boolean(value);
    if (state.messageSoundEnabled) ensureNotificationAudio();
    persist();
  }

  function callUserVolume(username) {
    const key = sanitizeUsername(username);
    if (!key) return 100;
    const value = Number(state.callUserVolumes[key]);
    return Number.isFinite(value) ? Math.max(0, Math.min(100, value)) : 100;
  }

  function setCallUserVolume(username, value) {
    const key = sanitizeUsername(username);
    if (!key) return;
    state.callUserVolumes[key] = Math.max(0, Math.min(100, Math.round(Number(value) || 0)));
    persist();
  }

  function ensureNotificationAudio() {
    try {
      const AudioCtx = window.AudioContext || window.webkitAudioContext;
      if (!AudioCtx) return null;
      if (!notificationAudioContext) notificationAudioContext = new AudioCtx();
      if (notificationAudioContext.state === "suspended") {
        notificationAudioContext.resume().catch(() => {});
      }
      return notificationAudioContext;
    } catch {
      return null;
    }
  }

  function playMessageNotificationSound() {
    if (!state.messageSoundEnabled) return;
    const context = ensureNotificationAudio();
    if (!context) return;
    try {
      const now = context.currentTime;
      const oscillator = context.createOscillator();
      const gain = context.createGain();
      oscillator.type = "sine";
      oscillator.frequency.setValueAtTime(740, now);
      oscillator.frequency.exponentialRampToValueAtTime(980, now + 0.08);
      gain.gain.setValueAtTime(0.0001, now);
      gain.gain.exponentialRampToValueAtTime(0.08, now + 0.012);
      gain.gain.exponentialRampToValueAtTime(0.0001, now + 0.18);
      oscillator.connect(gain);
      gain.connect(context.destination);
      oscillator.start(now);
      oscillator.stop(now + 0.2);
    } catch {
      /* notification audio is best-effort */
    }
  }

  function audioConstraints() {
    return {
      audio: state.selectedAudioInputId
        ? { deviceId: { exact: state.selectedAudioInputId } }
        : true
    };
  }

  async function refreshAudioDevices() {
    if (!navigator.mediaDevices?.enumerateDevices) return;
    try {
      state.audioDevices = await navigator.mediaDevices.enumerateDevices();
    } catch {
      state.audioDevices = [];
    }
  }

  async function unlockAudioDevices() {
    if (!navigator.mediaDevices?.getUserMedia) {
      state.lastError = "Audio devices are not available in this browser.";
      return false;
    }

    state.audioDevicesLoading = true;
    try {
      const stream = await navigator.mediaDevices.getUserMedia({ audio: true, video: false });
      for (const track of stream.getTracks()) track.stop();
      state.audioDevicesPermission = "granted";
      await refreshAudioDevices();
      return true;
    } catch {
      state.audioDevicesPermission = "denied";
      state.lastError = "Microphone permission is required to list audio devices.";
      await refreshAudioDevices();
      return false;
    } finally {
      state.audioDevicesLoading = false;
    }
  }

  function setAudioInput(deviceId) {
    state.selectedAudioInputId = String(deviceId || "");
    if (state.micTestActive) stopMicTest();
    persist();
  }

  function setAudioOutput(deviceId) {
    state.selectedAudioOutputId = String(deviceId || "");
    applyAudioOutput(micTestAudio);
    persist();
  }

  function setMicrophoneThreshold(value) {
    const next = Math.max(0, Math.min(100, Number(value) || 0));
    state.microphoneThreshold = next;
    persist();
  }

  async function applyAudioOutput(audio) {
    if (!audio || !state.selectedAudioOutputId || typeof audio.setSinkId !== "function") return;
    try {
      await audio.setSinkId(state.selectedAudioOutputId);
    } catch {
      /* Output selection is browser/permission dependent. */
    }
  }

  function setupCallAnalyser(stream) {
    state.callAnalyser = null;
    state.callAnalyserData = null;
    try {
      const AudioCtx = window.AudioContext || window.webkitAudioContext;
      if (!AudioCtx) return;
      const context = new AudioCtx();
      const source = context.createMediaStreamSource(stream);
      const analyser = context.createAnalyser();
      analyser.fftSize = 1024;
      source.connect(analyser);
      state.callAnalyser = { context, analyser };
      state.callAnalyserData = new Uint8Array(analyser.fftSize);
    } catch {
      state.callAnalyser = null;
      state.callAnalyserData = null;
    }
  }

  function closeCallAnalyser() {
    const context = state.callAnalyser?.context;
    state.callAnalyser = null;
    state.callAnalyserData = null;
    if (context) context.close().catch(() => {});
  }

  function isAboveMicrophoneThreshold() {
    const threshold = Number(state.microphoneThreshold) || 0;
    if (threshold <= 0 || !state.callAnalyser?.analyser || !state.callAnalyserData) return true;
    state.callAnalyser.analyser.getByteTimeDomainData(state.callAnalyserData);
    return microphoneLevelFromSamples(state.callAnalyserData) >= threshold;
  }

  function stopMicTest() {
    if (micTestFrame) {
      cancelAnimationFrame(micTestFrame);
      micTestFrame = 0;
    }
    if (micTestAudio) {
      micTestAudio.pause();
      micTestAudio.srcObject = null;
      micTestAudio = null;
    }
    if (micTestStream) {
      for (const track of micTestStream.getTracks()) track.stop();
      micTestStream = null;
    }
    const context = micTestAnalyser?.context;
    micTestAnalyser = null;
    micTestAnalyserData = null;
    if (context) context.close().catch(() => {});
    state.micTestActive = false;
    state.micTestLoading = false;
    state.micTestLevel = 0;
    micTestSmoothedLevel = 0;
  }

  async function startMicTest() {
    if (state.micTestActive || state.micTestLoading) {
      stopMicTest();
      return;
    }
    if (!navigator.mediaDevices?.getUserMedia) {
      state.lastError = "Audio devices are not available in this browser.";
      return;
    }

    state.micTestLoading = true;
    try {
      const stream = await navigator.mediaDevices.getUserMedia(audioConstraints());
      state.audioDevicesPermission = "granted";
      await refreshAudioDevices();
      micTestStream = stream;

      const AudioCtx = window.AudioContext || window.webkitAudioContext;
      if (AudioCtx) {
        const context = new AudioCtx();
        const source = context.createMediaStreamSource(stream);
        const analyser = context.createAnalyser();
        analyser.fftSize = 2048;
        analyser.smoothingTimeConstant = 0.82;
        source.connect(analyser);
        micTestAnalyser = { context, analyser };
        micTestAnalyserData = new Uint8Array(analyser.fftSize);
      }

      micTestAudio = new Audio();
      micTestAudio.srcObject = stream;
      micTestAudio.volume = 0.75;
      await applyAudioOutput(micTestAudio);
      micTestAudio.play().catch(() => {});

      state.micTestActive = true;
      const tick = () => {
        if (!state.micTestActive || !micTestAnalyser?.analyser || !micTestAnalyserData) return;
        micTestAnalyser.analyser.getByteTimeDomainData(micTestAnalyserData);
        const nextLevel = microphoneLevelFromSamples(micTestAnalyserData);
        micTestSmoothedLevel = smoothLevel(micTestSmoothedLevel, nextLevel);
        state.micTestLevel = Math.round(micTestSmoothedLevel);
        micTestFrame = requestAnimationFrame(tick);
      };
      tick();
    } catch {
      state.audioDevicesPermission = "denied";
      state.lastError = "Microphone permission is required to test audio.";
      stopMicTest();
    } finally {
      state.micTestLoading = false;
    }
  }

  function touchRoom(roomId, message) {
    const id = sanitizeRoomId(roomId);
    if (!id) return;
    const existing = state.rooms.find((r) => r.roomId === id);
    const preview = message ? (message.kind === "voice" ? "Voice message" : message.text || "") : "";
    const sender = message ? message.username : "";
    const ts = message ? message.timestamp : Date.now();

    if (existing) {
      if (ts >= (existing.lastTimestamp || 0)) {
        existing.lastPreview = preview || existing.lastPreview;
        existing.lastTimestamp = ts;
        existing.lastSender = sender || existing.lastSender;
      }
    } else {
      state.rooms.push({ roomId: id, lastPreview: preview, lastTimestamp: ts, lastSender: sender });
    }
    persist();
  }

  function clearRoomMessages(roomId) {
    const id = sanitizeRoomId(roomId);
    if (!id) return;
    for (const message of state.messagesByRoom[id] || []) {
      try {
        const url = attachmentUrlCache.get(message.messageId);
        if (url) URL.revokeObjectURL(url);
      } catch { /* ignore */ }
      attachmentUrlCache.delete(message.messageId);
    }
    delete state.messagesByRoom[id];
    delete state.unreadByRoom[id];
    const room = state.rooms.find((r) => r.roomId === id);
    if (room) {
      room.lastPreview = "";
      room.lastSender = "";
      room.lastTimestamp = Date.now();
    }
    persist();
  }

  function removeRoom(roomId) {
    const id = sanitizeRoomId(roomId);
    if (state.joinedRooms.includes(id)) {
      send({ op: 4, d: { gameId: id } });
    }
    state.rooms = state.rooms.filter((r) => r.roomId !== id);
    delete state.messagesByRoom[id];
    delete state.usersByRoom[id];
    delete state.unreadByRoom[id];
    state.joinedRooms = state.joinedRooms.filter((r) => r !== id);
    state.pendingJoinRooms = state.pendingJoinRooms.filter((r) => r !== id);
    if (state.activeRoom === id) state.activeRoom = "";
    persist();
  }

  function clearHeartbeat() {
    if (state.heartbeatTimer) {
      clearInterval(state.heartbeatTimer);
      state.heartbeatTimer = null;
    }
  }

  function startHeartbeat() {
    clearHeartbeat();
    state.heartbeatTimer = setInterval(() => {
      send({ op: 1, d: {} });
    }, state.heartbeatInterval);
  }

  function scrollToBottom() {
    nextTick(() => {
      const feed = document.querySelector(".feed");
      if (feed) feed.scrollTop = feed.scrollHeight;
    });
  }

  function teardownConnection(message) {
    clearHeartbeat();
    state.connected = false;
    state.identified = false;
    state.uuid = null;
    state.joinedRooms = [];
    state.pendingJoinRooms = [];
    state.usersByRoom = {};
    state.ws = null;
    if (message) state.systemBanner = message;
  }

  function send(payload) {
    if (!state.ws || state.ws.readyState !== WebSocket.OPEN) return;
    state.ws.send(JSON.stringify(payload));
  }

  function requestJoin(roomId) {
    const id = sanitizeRoomId(roomId);
    const validation = validateRoomId(id);
    if (validation) {
      state.lastError = validation;
      return;
    }
    if (!state.identified) return;
    if (state.joinedRooms.includes(id)) return;
    if (state.pendingJoinRooms.includes(id)) return;
    state.pendingJoinRooms.push(id);
    send({ op: 3, d: { gameId: id } });
  }

  function fetchHistory(roomId) {
    const id = sanitizeRoomId(roomId);
    if (!id || !isValidRoomId(id)) return;
    send({ op: 18, d: { gameId: id } });
  }

  function selectConversation(roomId) {
    const id = sanitizeRoomId(roomId);
    const validation = validateRoomId(id);
    if (validation) {
      state.lastError = validation;
      showToast(validation);
      return;
    }
    state.activeRoom = id;
    state.unreadByRoom[id] = 0;
    touchRoom(id);
    persist();
    if (state.connected && state.identified) {
      if (!state.joinedRooms.includes(id) && !state.pendingJoinRooms.includes(id)) {
        requestJoin(id);
      } else {
        scrollToBottom();
      }
    }
  }

  function leaveRoom(roomId) {
    const id = sanitizeRoomId(roomId || state.activeRoom);
    if (!id || !isValidRoomId(id)) return;

    if (state.connected && state.identified && state.joinedRooms.includes(id)) {
      send({ op: 4, d: { gameId: id } });
    }

    state.joinedRooms = state.joinedRooms.filter((r) => r !== id);
    state.pendingJoinRooms = state.pendingJoinRooms.filter((r) => r !== id);
    delete state.usersByRoom[id];
    if (state.deleteMessagesOnLeave) clearRoomMessages(id);
    if (state.activeRoom === id) state.activeRoom = "";
    persist();
  }

  function startCompose() {
    state.composing = true;
    state.composeInput = "";
  }

  function cancelCompose() {
    state.composing = false;
    state.composeInput = "";
  }

  function submitCompose() {
    const id = sanitizeRoomId(state.composeInput);
    const validation = validateRoomId(id);
    if (validation) {
      state.lastError = validation;
      showToast(validation);
      return;
    }
    state.composing = false;
    state.composeInput = "";
    selectConversation(id);
  }

  function showToast(message) {
    state.toastMessage = message;
    if (toastTimer) clearTimeout(toastTimer);
    toastTimer = setTimeout(() => {
      if (state.toastMessage === message) state.toastMessage = "";
      toastTimer = null;
    }, 2400);
  }

  async function createRandomRoom() {
    let id = "";
    try {
      id = generateRandomRoomId();
    } catch (error) {
      state.lastError = error.message;
      showToast("Could not generate a secure room.");
      return;
    }

    selectConversation(id);
    try {
      await copyTextToClipboard(id);
      showToast("Room ID copied.");
    } catch {
      showToast("Room opened. Copy failed.");
    }
  }

  function connect() {
    persist();
    if (state.connected || state.ws) return;
    const username = sanitizeUsername(state.username);
    if (!username) {
      state.lastError = "Nickname required.";
      return;
    }
    state.lastError = "";
    state.manualClose = false;
    try {
      state.ws = new WebSocket(inferWebSocketUrl());
    } catch (error) {
      state.lastError = `Connection failed: ${error.message}`;
      state.ws = null;
      return;
    }
    state.ws.addEventListener("open", () => {
      state.connected = true;
      state.systemBanner = "";
      send({
        op: 2,
        d: {
          username,
          isVoiceChat: state.voiceEnabled,
          v: "qxprotocol-web-vite-vue",
          isMobile: /Android|iPhone|iPad|iPod/i.test(navigator.userAgent),
          isSecure: window.isSecureContext
        }
      });
    });
    state.ws.addEventListener("message", ({ data }) => {
      try {
        handleMessage(JSON.parse(data));
      } catch {
        state.lastError = "Malformed payload.";
      }
    });
    state.ws.addEventListener("close", () => {
      teardownConnection(state.manualClose ? "" : "Connection lost");
    });
    state.ws.addEventListener("error", () => {
      state.lastError = "WebSocket error.";
    });
  }

  function disconnect() {
    state.manualClose = true;
    if (state.ws && state.ws.readyState < WebSocket.CLOSING) state.ws.close();
    teardownConnection("");
  }

  function sendChat() {
    const text = state.messageInput.trim();
    const roomId = state.activeRoom;
    if (!text || !roomId) return;
    if (state.connected && state.identified && state.joinedRooms.includes(roomId)) {
      const d = { text: text.slice(0, MESSAGE_LIMIT), gameId: roomId };
      if (state.replyingTo?.messageId) d.replyToMessageId = state.replyingTo.messageId;
      send({ op: 7, d });
      state.messageInput = "";
      state.replyingTo = null;
    } else {
      state.lastError = "Not joined to this room yet.";
    }
  }

  async function sendAttachment(file, caption = "") {
    const roomId = state.activeRoom;
    if (!file || !roomId) return;
    if (!state.connected || !state.identified) {
      state.lastError = "Not connected.";
      return;
    }
    if (!state.joinedRooms.includes(roomId)) {
      state.lastError = "Not joined to this room yet.";
      return;
    }
    if (file.size > MAX_ATTACHMENT_BYTES) {
      state.lastError = `File too large: ${file.name} (${formatSize(file.size)} > 10 MB)`;
      return;
    }

    try {
      const dataB64 = await blobToBase64(file);
      send({
        op: 7,
        d: {
          text: caption ? String(caption).trim().slice(0, MESSAGE_LIMIT) : "",
          gameId: roomId,
          ...(state.replyingTo?.messageId ? { replyToMessageId: state.replyingTo.messageId } : {}),
          attachment: {
            filename: String(file.name || "file").slice(0, 128),
            mimeType: file.type || "application/octet-stream",
            size: file.size,
            dataB64
          }
        }
      });
      state.replyingTo = null;
    } catch (err) {
      state.lastError = `Upload failed: ${err.message || err}`;
    }
  }

  // Voice memo: hold-to-record, release-to-send attachment.
  // Reuses the in-call mic stream when a call is active so the same mic
  // can serve both the call and the memo without a second getUserMedia.
  async function startRecordingVoiceMemo() {
    if (state.recording) return;
    const roomId = state.activeRoom;
    if (!roomId || !state.joinedRooms.includes(roomId)) {
      state.lastError = "Join a room first.";
      return;
    }
    try {
      const reusingCallStream = !!state.callStream;
      const stream = reusingCallStream
        ? state.callStream
        : await navigator.mediaDevices.getUserMedia(audioConstraints());
      refreshAudioDevices();
      state.audioDevicesPermission = "granted";
      const mimeType = pickAudioMime();
      const recorder = new MediaRecorder(stream, mimeType ? { mimeType } : undefined);
      const chunks = [];
      recorder.ondataavailable = (e) => { if (e.data && e.data.size) chunks.push(e.data); };
      recorder.onerror = () => { state.lastError = "Recording error."; };
      recorder.start();
      state.recording = {
        recorder,
        stream,
        roomId,
        chunks,
        startedAt: Date.now(),
        mimeType,
        ownsStream: !reusingCallStream
      };
      tickRecording();
    } catch (err) {
      state.lastError = "Mic access denied.";
    }
  }

  function tickRecording() {
    if (!state.recording) return;
    state.recordingElapsed = Math.floor((Date.now() - state.recording.startedAt) / 1000);
    setTimeout(tickRecording, 250);
  }

  async function stopRecordingVoiceMemo(cancel = false) {
    const rec = state.recording;
    if (!rec) return;
    state.recording = null;
    state.recordingElapsed = 0;
    try {
      await new Promise((resolve) => {
        rec.recorder.onstop = resolve;
        try { rec.recorder.stop(); } catch { resolve(); }
      });
    } catch { /* ignore */ }
    // Only stop tracks we started ourselves — never kill the call's stream.
    if (rec.ownsStream) {
      for (const t of rec.stream.getTracks()) t.stop();
    }
    if (cancel) return;
    if (!rec.chunks.length) return;

    const blob = new Blob(rec.chunks, { type: rec.mimeType || "audio/webm" });
    if (blob.size < 200) return;
    const ext = (rec.mimeType || "audio/webm").includes("ogg") ? "ogg" : "webm";
    const file = new File([blob], `voice-${Date.now()}.${ext}`, { type: blob.type });
    await sendAttachment(file, `[voice:${formatDuration(Math.floor(blob.size / 6000))}]`);
  }

  function pickAudioMime() {
    if (typeof MediaRecorder === "undefined") return "";
    const candidates = [
      "audio/webm;codecs=opus",
      "audio/webm",
      "audio/ogg;codecs=opus",
      "audio/mp4"
    ];
    for (const c of candidates) {
      if (MediaRecorder.isTypeSupported && MediaRecorder.isTypeSupported(c)) return c;
    }
    return "";
  }

  function formatDuration(seconds) {
    const m = Math.floor(seconds / 60);
    const s = Math.max(0, seconds % 60);
    return `${m}:${String(s).padStart(2, "0")}`;
  }

  // Voice call: toggle — announces via op 98 and streams chunked audio via op 99.
  async function startCall() {
    if (state.inCall) return;
    const roomId = state.activeRoom;
    if (!roomId || !state.joinedRooms.includes(roomId)) {
      state.lastError = "Join a room first.";
      return;
    }
    try {
      const stream = await navigator.mediaDevices.getUserMedia(audioConstraints());
      refreshAudioDevices();
      state.audioDevicesPermission = "granted";
      setupCallAnalyser(stream);
      state.callStream = stream;
      state.callRoom = roomId;
      state.inCall = true;
      state.callMuted = false;
      state.voiceEnabled = true;
      // Register self as voice member locally so our tile shows up immediately.
      const me = sanitizeUsername(state.username);
      if (me) {
        const members = new Set(state.voiceMembersByRoom[roomId] || []);
        members.add(me);
        state.voiceMembersByRoom[roomId] = [...members];
      }
      send({ op: 98, d: { isVoiceChat: true } });
      scheduleCallChunk();
      tickCall(Date.now());
    } catch (err) {
      state.lastError = "Mic access denied.";
      endCall();
    }
  }

  function toggleMute() {
    if (!state.callStream) return;
    state.callMuted = !state.callMuted;
    for (const track of state.callStream.getAudioTracks()) {
      track.enabled = !state.callMuted;
    }
  }

  function scheduleCallChunk() {
    if (!state.inCall || !state.callStream) return;
    const mimeType = pickAudioMime();
    let recorder;
    try {
      recorder = new MediaRecorder(state.callStream, mimeType ? { mimeType } : undefined);
    } catch {
      endCall();
      return;
    }
    const chunks = [];
    recorder.ondataavailable = (e) => { if (e.data && e.data.size) chunks.push(e.data); };
    recorder.onstop = async () => {
      if (chunks.length && state.inCall && state.connected && isAboveMicrophoneThreshold()) {
        try {
          const blob = new Blob(chunks, { type: mimeType || "audio/webm" });
          const b64 = await blobToBase64(blob);
          send({
            op: 99,
            d: {
              gameId: state.callRoom,
              chunk: b64,
              mimeType: mimeType || "audio/webm"
            }
          });
        } catch { /* ignore chunk */ }
      }
      if (state.inCall) scheduleCallChunk();
    };
    state.callRecorder = recorder;
    recorder.start();
    state.callTimer = setTimeout(() => {
      if (recorder.state === "recording") {
        try { recorder.stop(); } catch { /* ignore */ }
      }
    }, CALL_CHUNK_MS);
  }

  function tickCall(startedAt) {
    if (!state.inCall) { state.callElapsed = 0; return; }
    state.callElapsed = Math.floor((Date.now() - startedAt) / 1000);
    setTimeout(() => tickCall(startedAt), 500);
  }

  function endCall() {
    const roomId = state.callRoom;
    if (state.callTimer) { clearTimeout(state.callTimer); state.callTimer = null; }
    if (state.callRecorder) {
      try { state.callRecorder.stop(); } catch { /* ignore */ }
      state.callRecorder = null;
    }
    if (state.callStream) {
      // If a memo is also recording off the same stream, let it finish first.
      const memoUsesStream = state.recording && !state.recording.ownsStream;
      if (!memoUsesStream) {
        for (const t of state.callStream.getTracks()) t.stop();
      }
      state.callStream = null;
    }
    closeCallAnalyser();
    if (state.inCall) send({ op: 98, d: { isVoiceChat: false } });
    state.inCall = false;
    state.voiceEnabled = false;
    state.callRoom = "";
    state.callElapsed = 0;
    state.callMuted = false;

    // Remove self from voice members locally.
    const me = sanitizeUsername(state.username);
    if (me && roomId) {
      const members = (state.voiceMembersByRoom[roomId] || []).filter((u) => u !== me);
      state.voiceMembersByRoom[roomId] = members;
    }
  }

  function handleIncomingCallChunk(d, fromUser) {
    if (!d?.chunk) return;
    const roomId = sanitizeRoomId(d.gameId || state.activeRoom);
    try {
      const blob = base64ToBlob(d.chunk, d.mimeType || "audio/webm");
      const url = URL.createObjectURL(blob);
      const audio = new Audio(url);
      audio.volume = Math.max(0, Math.min(1, callUserVolume(fromUser) / 100));
      applyAudioOutput(audio);
      audio.play().catch(() => { /* autoplay may be blocked before first gesture */ });
      audio.onended = () => URL.revokeObjectURL(url);
      if (fromUser && roomId) {
        if (!state.speakingByRoom[roomId]) state.speakingByRoom[roomId] = {};
        state.speakingByRoom[roomId][fromUser] = Date.now();
        // Voice members inferred from activity — register even if we missed op 98
        const members = state.voiceMembersByRoom[roomId] || [];
        if (!members.includes(fromUser)) state.voiceMembersByRoom[roomId] = [...members, fromUser];
      }
    } catch { /* ignore chunk decode errors */ }
  }

  function handleVoiceState(d) {
    const roomId = sanitizeRoomId(d?.gameId);
    if (!roomId) return;
    const user = d?.user;
    if (!user || d?.ok) return;    // our own op 98 ack has {ok} but no user — skip
    const members = new Set(state.voiceMembersByRoom[roomId] || []);
    if (d.isVoiceChat === true) members.add(user);
    else members.delete(user);
    state.voiceMembersByRoom[roomId] = [...members];
  }

  // Keep op 98 as a manual toggle for backward compat (affects voice eligibility).
  function toggleVoice() {
    if (state.inCall) {
      endCall();
    } else {
      startCall();
    }
  }

  function changeUsername(newName) {
    const clean = sanitizeUsername(newName);
    if (!clean) { state.lastError = "Name cannot be empty."; return false; }
    if (clean === sanitizeUsername(state.username)) return true;
    const wasConnected = state.connected;
    state.username = clean;
    persist();
    if (wasConnected) {
      disconnect();
      setTimeout(connect, 100);
    }
    return true;
  }

  function clearAllData() {
    if (state.inCall) endCall();
    if (state.recording) stopRecordingVoiceMemo(true);
    disconnect();
    for (const url of attachmentUrlCache.values()) {
      try { URL.revokeObjectURL(url); } catch { /* ignore */ }
    }
    attachmentUrlCache.clear();
    state.rooms = [];
    state.messagesByRoom = {};
    state.unreadByRoom = {};
    state.usersByRoom = {};
    state.activeRoom = "";
    state.joinedRooms = [];
    state.pendingJoinRooms = [];
    persist();
  }

  function toggleReaction(message, emoji) {
    if (state.connected && state.identified && message?.messageId) {
      send({
        op: 19,
        d: { messageId: message.messageId, reaction: emoji, gameId: message.roomId || state.activeRoom }
      });
    }
  }

  function deleteMessage(message) {
    if (!state.connected || !state.identified || !message?.messageId) return;
    const gameId = message.roomId || state.activeRoom;
    if (!gameId) return;
    send({ op: 21, d: { messageId: message.messageId, gameId } });
  }

  function findMessageById(roomId, messageId) {
    const id = sanitizeRoomId(roomId || state.activeRoom);
    const target = String(messageId || "");
    if (!id || !target) return null;
    return (state.messagesByRoom[id] || []).find((message) => message.messageId === target) || null;
  }

  function startReply(message) {
    if (!message?.messageId || message.deleted) return;
    state.replyingTo = {
      messageId: message.messageId,
      roomId: message.roomId || state.activeRoom,
      username: message.username || "",
      text: message.kind === "image" ? "Photo" : message.kind === "file" ? "File attachment" : message.text || ""
    };
  }

  function cancelReply() {
    state.replyingTo = null;
  }

  function applyPreview(payload) {
    const messageId = payload?.messageId;
    const targetRoom = sanitizeRoomId(payload?.gameId || "");
    const preview = payload?.preview;
    if (!messageId || !preview || typeof preview !== "object") return;
    const rooms = targetRoom ? [targetRoom] : Object.keys(state.messagesByRoom);
    for (const id of rooms) {
      const arr = state.messagesByRoom[id];
      if (!arr) continue;
      const index = arr.findIndex((m) => m.messageId === messageId);
      if (index === -1) continue;
      if (arr[index].deleted) return;
      arr[index] = normalizeMessage({ ...arr[index], preview }, id);
      persist();
      return;
    }
  }

  function applyDeletion(payload) {
    const messageId = payload?.messageId;
    if (!messageId) return;
    const targetRoom = sanitizeRoomId(payload?.gameId || "");
    const rooms = targetRoom ? [targetRoom] : Object.keys(state.messagesByRoom);
    for (const id of rooms) {
      const arr = state.messagesByRoom[id];
      if (!arr) continue;
      const index = arr.findIndex((m) => m.messageId === messageId);
      if (index === -1) continue;
      // Drop any associated Blob URL so it can be GC'd.
      try { attachmentUrlCache.get(messageId) && URL.revokeObjectURL(attachmentUrlCache.get(messageId)); } catch { /* ignore */ }
      attachmentUrlCache.delete(messageId);
      arr[index] = normalizeMessage({
        ...arr[index],
        text: "",
        attachment: null,
        preview: null,
        reactions: [],
        deleted: true
      }, id);
      persist();
      return;
    }
  }

  function pushMessageToRoom(roomId, normalized) {
    const id = sanitizeRoomId(roomId);
    if (!id) return false;
    if (!state.messagesByRoom[id]) state.messagesByRoom[id] = [];
    const arr = state.messagesByRoom[id];
    const index = arr.findIndex((m) => m.messageId === normalized.messageId);
    if (index === -1) {
      arr.push(normalized);
      if (arr.length > MAX_HISTORY_PER_ROOM) arr.splice(0, arr.length - MAX_HISTORY_PER_ROOM);
      return true;
    } else {
      arr[index] = normalized;
      return false;
    }
  }

  function upsertMessage(message) {
    const roomId = sanitizeRoomId(message.roomId || state.activeRoom);
    const normalized = normalizeMessage(message, roomId);
    const added = pushMessageToRoom(roomId, normalized);
    touchRoom(roomId, normalized);

    const mine = isOwnMessage(normalized);
    if (added && !mine) playMessageNotificationSound();
    if (!mine && roomId !== state.activeRoom) {
      state.unreadByRoom[roomId] = (state.unreadByRoom[roomId] || 0) + 1;
    }

    if (roomId === state.activeRoom) scrollToBottom();
    persist();
  }

  function applyReactions(payload) {
    const messageId = payload?.messageId;
    const targetRoom = sanitizeRoomId(payload?.roomId || "");
    const rooms = targetRoom ? [targetRoom] : Object.keys(state.messagesByRoom);
    for (const id of rooms) {
      const arr = state.messagesByRoom[id];
      if (!arr) continue;
      const index = arr.findIndex((m) => m.messageId === messageId);
      if (index !== -1) {
        arr[index] = normalizeMessage({ ...arr[index], reactions: payload.reactions || [] }, id);
        persist();
        return;
      }
    }
  }

  function handleMessage(message) {
    const { op, d } = message;
    switch (op) {
      case 0:
        if (d?.error) state.lastError = d.error;
        break;
      case 1:
        /* heartbeat ack — ignored */
        break;
      case 2:
        state.uuid = d.uuid;
        state.identified = true;
        state.systemBanner = "";
        for (const r of state.rooms) requestJoin(r.roomId);
        break;
      case 3:
        handleJoinOp(d);
        break;
      case 4:
        handleLeaveOp(d);
        break;
      case 7:
        if (d?.error) {
          state.lastError = d.error;
        } else if (d?.messageId && typeof d?.timestamp === "number") {
          // Full broadcast frame ({messageId, text, username, timestamp, ...}).
          upsertMessage(d);
        }
        // else: {messageId, ok: true} sender-ack — ignored, we already got the broadcast.
        break;
      case 10:
        if (d?.heartbeat_interval) {
          state.heartbeatInterval = d.heartbeat_interval;
          startHeartbeat();
        }
        break;
      case 18:
        handleHistoryOp(d);
        break;
      case 20:
        applyReactions(d);
        break;
      case 22:
        applyDeletion(d);
        break;
      case 23:
        applyPreview(d);
        break;
      case 24:
        state.lastError = d?.reason ? `Blacklisted: ${d.reason}` : "Blacklisted.";
        disconnect();
        break;
      case 87:
        state.systemBanner = d?.msg || state.systemBanner;
        break;
      case 98:
        handleVoiceState(d);
        break;
      case 99:
        handleIncomingCallChunk(d, message.u);
        break;
      default:
        break;
    }
  }

  function handleJoinOp(d) {
    const roomId = sanitizeRoomId(d?.gameId);
    if (!roomId) return;

    if (Array.isArray(d?.players)) {
      state.usersByRoom[roomId] = d.players;
    }
    if (Array.isArray(d?.voicePlayers)) {
      state.voiceMembersByRoom[roomId] = d.voicePlayers;
    }

    if (d?.ok && !d?.system) {
      if (!state.joinedRooms.includes(roomId)) state.joinedRooms.push(roomId);
      state.pendingJoinRooms = state.pendingJoinRooms.filter((r) => r !== roomId);
      touchRoom(roomId);
      if (roomId === state.activeRoom) fetchHistory(roomId);
      else if (!state.messagesByRoom[roomId]?.length) fetchHistory(roomId);
    }
  }

  function handleLeaveOp(d) {
    const roomId = sanitizeRoomId(d?.gameId);
    if (!roomId) return;

    if (d?.ok) {
      state.joinedRooms = state.joinedRooms.filter((r) => r !== roomId);
      if (state.deleteMessagesOnLeave) clearRoomMessages(roomId);
      if (roomId === state.activeRoom) state.activeRoom = "";
    } else if (d?.left) {
      const arr = state.usersByRoom[roomId];
      if (arr) state.usersByRoom[roomId] = arr.filter((u) => u !== d.left);
    }
  }

  function handleHistoryOp(d) {
    if (!d?.ok) return;
    const roomId = sanitizeRoomId(d.roomId);
    if (!roomId) return;
    const messages = Array.isArray(d.messages) ? d.messages.map((m) => normalizeMessage(m, roomId)) : [];
    state.messagesByRoom[roomId] = messages;
    const last = messages[messages.length - 1];
    if (last) touchRoom(roomId, last);
    if (roomId === state.activeRoom) scrollToBottom();
    persist();
  }

  function isOwnMessage(message) {
    const me = sanitizeUsername(state.username);
    return !!me && me === message.username;
  }

  function exportData() {
    const payload = {
      version: 4,
      exportedAt: new Date().toISOString(),
      username: state.username,
      activeRoom: state.activeRoom,
      rooms: state.rooms,
      messagesByRoom: state.messagesByRoom,
      unreadByRoom: state.unreadByRoom,
      deleteMessagesOnLeave: state.deleteMessagesOnLeave,
      streamerMode: state.streamerMode,
      messageSoundEnabled: state.messageSoundEnabled,
      callUserVolumes: sanitizeCallUserVolumes(state.callUserVolumes)
    };
    const blob = new Blob([JSON.stringify(payload, null, 2)], { type: "application/json" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    const stamp = new Date().toISOString().replace(/[:T]/g, "-").slice(0, 19);
    a.href = url;
    a.download = `qxprotocol-backup-${stamp}.json`;
    document.body.appendChild(a);
    a.click();
    a.remove();
    URL.revokeObjectURL(url);
    state.settingsOpen = false;
    state.systemBanner = "Backup exported.";
    setTimeout(() => {
      if (state.systemBanner === "Backup exported.") state.systemBanner = "";
    }, 2000);
  }

  function importData(file) {
    state.settingsOpen = false;
    if (!file) return;
    const reader = new FileReader();
    reader.onerror = () => { state.lastError = "Couldn't read file."; };
    reader.onload = (e) => {
      try {
        const data = JSON.parse(String(e.target.result));
        if (!data || typeof data !== "object") throw new Error("Not an object");
        const usedToBeConnected = state.connected;
        if (usedToBeConnected) disconnect();

        if (typeof data.username === "string") state.username = sanitizeUsername(data.username);

        if (Array.isArray(data.rooms)) {
          state.rooms = data.rooms
            .filter((r) => r && typeof r.roomId === "string")
            .slice(0, MAX_ROOMS_SHOWN)
            .map((r) => ({
              roomId: sanitizeRoomId(r.roomId),
              lastPreview: String(r.lastPreview || ""),
              lastTimestamp: Number(r.lastTimestamp) || 0,
              lastSender: String(r.lastSender || "")
            }))
            .filter((r) => isValidRoomId(r.roomId));
        }

        if (data.messagesByRoom && typeof data.messagesByRoom === "object") {
          const next = {};
          for (const [id, arr] of Object.entries(data.messagesByRoom)) {
            if (!Array.isArray(arr)) continue;
            const roomId = sanitizeRoomId(id);
            if (!isValidRoomId(roomId)) continue;
            next[roomId] = arr
              .slice(-MAX_HISTORY_PER_ROOM)
              .map((m) => normalizeMessage(m, id));
          }
          state.messagesByRoom = next;
        }

        if (data.unreadByRoom && typeof data.unreadByRoom === "object") {
          const next = {};
          for (const [id, n] of Object.entries(data.unreadByRoom)) {
            const v = Number(n);
            const roomId = sanitizeRoomId(id);
            if (Number.isFinite(v) && v > 0 && isValidRoomId(roomId)) next[roomId] = v;
          }
          state.unreadByRoom = next;
        }

        if (typeof data.activeRoom === "string") {
          state.activeRoom = isValidRoomId(data.activeRoom) ? sanitizeRoomId(data.activeRoom) : "";
        }
        if (typeof data.deleteMessagesOnLeave === "boolean") state.deleteMessagesOnLeave = data.deleteMessagesOnLeave;
        if (typeof data.streamerMode === "boolean") state.streamerMode = data.streamerMode;
        if (typeof data.messageSoundEnabled === "boolean") state.messageSoundEnabled = data.messageSoundEnabled;
        state.callUserVolumes = sanitizeCallUserVolumes(data.callUserVolumes);

        persist();
        state.lastError = "";
        state.systemBanner = "Backup imported.";
        setTimeout(() => {
          if (state.systemBanner === "Backup imported.") state.systemBanner = "";
        }, 2000);

        if (usedToBeConnected) connect();
      } catch (err) {
        state.lastError = `Import failed: ${err.message}`;
      }
    };
    reader.readAsText(file);
  }

  singleton = {
    QUICK_REACTIONS,
    MESSAGE_LIMIT,
    state,
    roomTitle,
    roomLabel,
    connectionLabel,
    onlineCount,
    canSend,
    sortedMessages,
    conversations,
    activeConversation,
    memberRoster,
    accentFor,
    formatTime,
    formatDay,
    formatSize,
    formatDuration,
    buildWaveform,
    attachmentUrlFor,
    displayRoomName,
    validateRoomId,
    isValidRoomId,

    persist,
    refreshAudioDevices,
    unlockAudioDevices,
    startMicTest,
    stopMicTest,
    setAudioInput,
    setAudioOutput,
    setMicrophoneThreshold,
    setDeleteMessagesOnLeave,
    setStreamerMode,
    setMessageSoundEnabled,
    callUserVolume,
    setCallUserVolume,
    applyAudioOutput,
    connect,
    disconnect,
    selectConversation,
    leaveRoom,
    sendChat,
    sendAttachment,
    startRecordingVoiceMemo,
    stopRecordingVoiceMemo,
    startCall,
    endCall,
    toggleMute,
    toggleVoice,
    toggleReaction,
    deleteMessage,
    findMessageById,
    startReply,
    cancelReply,
    fetchHistory,
    isOwnMessage,
    startCompose,
    cancelCompose,
    submitCompose,
    createRandomRoom,
    removeRoom,
    touchRoom,
    exportData,
    importData,
    changeUsername,
    clearAllData
  };

  return singleton;
}
