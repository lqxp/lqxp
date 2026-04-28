<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref } from "vue";

const props = defineProps({
  messenger: { type: Object, required: true }
});

const composerRef = ref<HTMLElement | null>(null);
const fileInputRef = ref<HTMLInputElement | null>(null);
const inputRef = ref<HTMLInputElement | null>(null);
const emojiWrapRef = ref<HTMLElement | null>(null);
const cameraVideoRef = ref<HTMLVideoElement | null>(null);
const cameraCanvasRef = ref<HTMLCanvasElement | null>(null);
const pickerOpen = ref(false);
const cameraOpen = ref(false);
const cameraBusy = ref(false);
const cameraError = ref("");
let cameraStream: MediaStream | null = null;

const canSend = computed(() => props.messenger.state.messageInput.trim().length > 0 && !!props.messenger.state.activeRoom);
const disabled = computed(() => !props.messenger.state.activeRoom);
const editing = computed(() => !!props.messenger.state.editingMessage);
const mediaDisabled = computed(() => disabled.value || editing.value);
const recording = computed(() => !!props.messenger.state.recording);

// Curated emoji palette — intentionally compact (80 glyphs) so it fits one screenful
// without needing tabs/search.
const EMOJIS = [
  "😀","😂","🤣","😊","😍","🥰","😘","😎","🤩","😇",
  "🙂","😉","😋","😛","😜","🤪","🤗","🤭","🤔","🧐",
  "😏","🙄","😬","😒","😞","😔","😢","😭","😤","😡",
  "🥺","😳","😱","😴","🤒","🤕","🤧","🥳","🤯","💀",
  "👍","👎","👌","✌️","🤞","🤘","🤙","👏","🙏","🤝",
  "💪","👀","👋","🙌","🤦","🤷","💃","🕺","🦾","🧠",
  "❤️","🧡","💛","💚","💙","💜","🖤","🤍","💔","💘",
  "🔥","✨","⭐","🎉","🎊","💯","💢","💥","💫","☕"
];

function pastedExtension(mimeType) {
  const type = String(mimeType || "").toLowerCase().split(";")[0];
  const known = {
    "application/gzip": "gz",
    "application/pdf": "pdf",
    "application/zip": "zip",
    "audio/mpeg": "mp3",
    "audio/ogg": "ogg",
    "audio/wav": "wav",
    "image/gif": "gif",
    "image/jpeg": "jpg",
    "image/png": "png",
    "image/webp": "webp",
    "text/plain": "txt",
    "video/mp4": "mp4",
    "video/webm": "webm"
  };
  if (known[type]) return known[type];
  const subtype = type.includes("/") ? type.split("/").pop() : "";
  const clean = String(subtype || "").replace(/[^a-z0-9]/g, "");
  return clean || "bin";
}

function namePastedFile(file: File, index: number) {
  if (file.name) return file;
  const filename = `pasted-${Date.now()}-${index + 1}.${pastedExtension(file.type)}`;
  try {
    return new File([file], filename, {
      type: file.type || "application/octet-stream",
      lastModified: file.lastModified || Date.now()
    });
  } catch {
    return file;
  }
}

function filesFromClipboard(event: ClipboardEvent): File[] {
  const clipboard = event.clipboardData;
  if (!clipboard) return [];

  const directFiles = Array.from(clipboard.files || []);
  const files = directFiles.length
    ? directFiles
    : Array.from((clipboard.items || []) as DataTransferItemList)
        .filter((item) => item.kind === "file")
        .map((item) => item.getAsFile())
        .filter((file): file is File => Boolean(file));

  return files.map(namePastedFile);
}

function isEditableElement(element: Element | null) {
  if (!element || element === document.body || element === document.documentElement) return false;
  if (element instanceof HTMLElement && element.isContentEditable) return true;
  return ["INPUT", "TEXTAREA", "SELECT"].includes(element.tagName);
}

async function onPaste(event: ClipboardEvent) {
  if (mediaDisabled.value || recording.value) return;
  const files = filesFromClipboard(event);
  if (!files.length) return;

  const target = event.target;
  const isComposerPaste = target instanceof Node && !!composerRef.value?.contains(target);
  if (!isComposerPaste && isEditableElement(document.activeElement)) return;

  event.preventDefault();
  pickerOpen.value = false;
  for (const file of files) {
    await props.messenger.sendAttachment(file);
  }
}

function send() {
  if (!canSend.value) return;
  props.messenger.sendChat();
}

function pickFile() {
  if (mediaDisabled.value) return;
  fileInputRef.value?.click();
}

async function pickCamera() {
  if (mediaDisabled.value) return;
  await openCamera();
}

async function onFile(event: Event) {
  const input = event.target as HTMLInputElement;
  const files = Array.from(input.files || []);
  for (const f of files) {
    await props.messenger.sendAttachment(f);
  }
  input.value = "";
}

function startHold() {
  if (mediaDisabled.value || recording.value) return;
  props.messenger.startRecordingVoiceMemo();
}

function endHold() {
  if (!recording.value) return;
  props.messenger.stopRecordingVoiceMemo(false);
}

function cancelHold() {
  if (!recording.value) return;
  props.messenger.stopRecordingVoiceMemo(true);
}

function togglePicker() {
  if (disabled.value) return;
  pickerOpen.value = !pickerOpen.value;
}

async function insertEmoji(emoji: string) {
  const input = inputRef.value;
  const current = props.messenger.state.messageInput || "";

  if (!input) {
    props.messenger.state.messageInput = current + emoji;
    return;
  }

  const start = input.selectionStart ?? current.length;
  const end = input.selectionEnd ?? current.length;
  const before = current.slice(0, start);
  const after = current.slice(end);
  let next = before + emoji + after;
  // Respect the same character cap the composer enforces via maxlength.
  const limit = props.messenger.MESSAGE_LIMIT || 2000;
  if (next.length > limit) next = next.slice(0, limit);
  props.messenger.state.messageInput = next;

  await nextTick();
  input.focus();
  const pos = Math.min(next.length, before.length + emoji.length);
  try { input.setSelectionRange(pos, pos); } catch { /* some input types throw */ }
}

function onDocMouseDown(event: MouseEvent) {
  if (!pickerOpen.value) return;
  if (!emojiWrapRef.value) return;
  if (!(event.target instanceof Node) || !emojiWrapRef.value.contains(event.target)) pickerOpen.value = false;
}

function onDocKey(event: KeyboardEvent) {
  if (pickerOpen.value && event.key === "Escape") pickerOpen.value = false;
  if (cameraOpen.value && event.key === "Escape") closeCamera();
}

function stopCameraStream() {
  if (!cameraStream) return;
  for (const track of cameraStream.getTracks()) track.stop();
  cameraStream = null;
}

async function openCamera() {
  cameraError.value = "";
  if (!navigator.mediaDevices?.getUserMedia) {
    cameraError.value = "Camera is not available in this browser.";
    return;
  }

  cameraOpen.value = true;
  await nextTick();

  try {
    cameraStream = await navigator.mediaDevices.getUserMedia({
      video: { facingMode: { ideal: "environment" } },
      audio: false
    });
    if (cameraVideoRef.value) {
      cameraVideoRef.value.srcObject = cameraStream;
      await cameraVideoRef.value.play();
    }
  } catch {
    cameraError.value = "Camera access denied or unavailable.";
    stopCameraStream();
  }
}

function closeCamera() {
  stopCameraStream();
  cameraOpen.value = false;
  cameraBusy.value = false;
  cameraError.value = "";
}

async function capturePhoto() {
  const video = cameraVideoRef.value;
  const canvas = cameraCanvasRef.value;
  if (!video || !canvas || cameraBusy.value) return;

  const width = video.videoWidth || 1280;
  const height = video.videoHeight || 720;
  if (!width || !height) {
    cameraError.value = "Camera is not ready yet.";
    return;
  }

  cameraBusy.value = true;
  canvas.width = width;
  canvas.height = height;
  const ctx = canvas.getContext("2d");
  if (!ctx) {
    cameraError.value = "Could not capture photo.";
    cameraBusy.value = false;
    return;
  }
  ctx.drawImage(video, 0, 0, width, height);

  const blob = await new Promise<Blob | null>((resolve) => canvas.toBlob(resolve, "image/jpeg", 0.9));
  if (!blob) {
    cameraError.value = "Could not capture photo.";
    cameraBusy.value = false;
    return;
  }

  const file = new File([blob], `camera-${Date.now()}.jpg`, { type: "image/jpeg" });
  await props.messenger.sendAttachment(file);
  closeCamera();
}

onMounted(() => {
  document.addEventListener("mousedown", onDocMouseDown);
  document.addEventListener("keydown", onDocKey);
  document.addEventListener("paste", onPaste);
});
onBeforeUnmount(() => {
  document.removeEventListener("mousedown", onDocMouseDown);
  document.removeEventListener("keydown", onDocKey);
  document.removeEventListener("paste", onPaste);
  stopCameraStream();
});
</script>

<template>
  <footer ref="composerRef" class="composer">
    <div v-if="recording" class="composer__recording">
      <span class="rec-dot"></span>
      <span class="rec-label">Recording</span>
      <span class="rec-time">{{ messenger.formatDuration(messenger.state.recordingElapsed) }}</span>
      <span class="rec-spacer"></span>
      <button type="button" class="btn--ghost" @click="cancelHold">cancel</button>
      <button type="button" class="btn btn--send" @click="endHold">send</button>
    </div>

    <template v-else>
      <input
        ref="fileInputRef"
        type="file"
        multiple
        style="display: none"
        @change="onFile"
      />
      <div v-if="messenger.state.editingMessage" class="reply-draft edit-draft">
        <div>
          <span class="reply-draft__label">Editing message</span>
          <span class="reply-draft__text">{{ messenger.state.editingMessage.text }}</span>
        </div>
        <button type="button" class="icon-btn" aria-label="Cancel edit" @click="messenger.cancelEditMessage">
          <svg viewBox="0 0 24 24"><path d="M18 6 6 18M6 6l12 12"/></svg>
        </button>
      </div>
      <div v-else-if="messenger.state.replyingTo" class="reply-draft">
        <div>
          <span class="reply-draft__label">Replying to {{ messenger.state.replyingTo.username || "message" }}</span>
          <span class="reply-draft__text">{{ messenger.state.replyingTo.text }}</span>
        </div>
        <button type="button" class="icon-btn" aria-label="Cancel reply" @click="messenger.cancelReply">
          <svg viewBox="0 0 24 24"><path d="M18 6 6 18M6 6l12 12"/></svg>
        </button>
      </div>
      <button class="icon-btn" type="button" aria-label="Attach file" :disabled="mediaDisabled" @click="pickFile">
        <svg viewBox="0 0 24 24"><path d="M21.44 11.05 12.25 20.24a6 6 0 0 1-8.49-8.49l9.19-9.19a4 4 0 1 1 5.66 5.66l-9.2 9.19a2 2 0 1 1-2.83-2.83L14.83 7"/></svg>
      </button>
      <button class="icon-btn" type="button" aria-label="Take photo" :disabled="mediaDisabled" @click="pickCamera">
        <svg viewBox="0 0 24 24"><path d="M4 7h3l1.4-2h7.2L17 7h3a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V9a2 2 0 0 1 2-2Z"/><circle cx="12" cy="13" r="3.5"/></svg>
      </button>

      <label class="composer__input">
        <textarea
          ref="inputRef"
          v-model="messenger.state.messageInput"
          :maxlength="messenger.MESSAGE_LIMIT"
          rows="1"
          :placeholder="disabled ? 'Join a room to start messaging' : editing ? 'Edit message' : 'Message'"
          :disabled="disabled"
          autocomplete="off"
          spellcheck="false"
          @keydown.enter.exact.prevent="send"
        ></textarea>
        <span class="composer__emoji-wrap" ref="emojiWrapRef">
          <button
            class="icon-btn"
            type="button"
            aria-label="Emoji"
            :aria-expanded="pickerOpen"
            :disabled="disabled"
            @click.prevent="togglePicker"
          >
            <svg viewBox="0 0 24 24"><circle cx="12" cy="12" r="10"/><path d="M8 14s1.5 2 4 2 4-2 4-2"/><line x1="9" y1="9" x2="9.01" y2="9"/><line x1="15" y1="9" x2="15.01" y2="9"/></svg>
          </button>

          <div v-if="pickerOpen" class="emoji-picker" role="menu">
            <button
              v-for="emoji in EMOJIS"
              :key="emoji"
              type="button"
              class="emoji-picker__cell"
              :aria-label="emoji"
              @click="insertEmoji(emoji)"
            >{{ emoji }}</button>
          </div>
        </span>
      </label>

      <button
        v-if="canSend"
        class="icon-btn composer__send"
        type="button"
        aria-label="Send"
        @click="send"
      >
        <svg viewBox="0 0 24 24"><path d="m22 2-7 20-4-9-9-4 20-7Z"/></svg>
      </button>
      <button
        v-else
        class="icon-btn composer__mic"
        type="button"
        aria-label="Hold to record voice"
        :disabled="mediaDisabled"
        @mousedown.prevent="startHold"
        @mouseup.prevent="endHold"
        @mouseleave="endHold"
        @touchstart.prevent="startHold"
        @touchend.prevent="endHold"
        @touchcancel.prevent="cancelHold"
      >
        <svg viewBox="0 0 24 24"><path d="M12 2a3 3 0 0 0-3 3v7a3 3 0 0 0 6 0V5a3 3 0 0 0-3-3Z"/><path d="M19 10a7 7 0 0 1-14 0"/><line x1="12" y1="19" x2="12" y2="23"/><line x1="8" y1="23" x2="16" y2="23"/></svg>
      </button>
    </template>
  </footer>

  <Teleport to="body">
    <div v-if="cameraOpen" class="camera-modal" role="dialog" aria-modal="true" aria-label="Take photo">
      <div class="camera-modal__panel">
        <header class="camera-modal__head">
          <span>Camera</span>
          <button type="button" class="icon-btn" aria-label="Close camera" @click="closeCamera">
            <svg viewBox="0 0 24 24"><path d="M18 6 6 18M6 6l12 12"/></svg>
          </button>
        </header>

        <div class="camera-modal__preview">
          <video
            ref="cameraVideoRef"
            autoplay
            muted
            playsinline
          ></video>
          <div v-if="cameraError" class="camera-modal__error">{{ cameraError }}</div>
        </div>

        <canvas ref="cameraCanvasRef" class="sr-only"></canvas>

        <div class="camera-modal__actions">
          <button type="button" class="btn" @click="closeCamera">Cancel</button>
          <button type="button" class="btn btn--primary" :disabled="cameraBusy || !!cameraError" @click="capturePhoto">
            {{ cameraBusy ? "Sending..." : "Take photo" }}
          </button>
        </div>
      </div>
    </div>
  </Teleport>
</template>
