<script setup>
import { computed, nextTick, onBeforeUnmount, onMounted, ref } from "vue";

const props = defineProps({
  messenger: { type: Object, required: true }
});

const fileInputRef = ref(null);
const inputRef = ref(null);
const emojiWrapRef = ref(null);
const cameraVideoRef = ref(null);
const cameraCanvasRef = ref(null);
const pickerOpen = ref(false);
const cameraOpen = ref(false);
const cameraBusy = ref(false);
const cameraError = ref("");
let cameraStream = null;

const canSend = computed(() => props.messenger.state.messageInput.trim().length > 0 && !!props.messenger.state.activeRoom);
const disabled = computed(() => !props.messenger.state.activeRoom);
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

function send() {
  if (!canSend.value) return;
  props.messenger.sendChat();
}

function pickFile() {
  if (disabled.value) return;
  fileInputRef.value?.click();
}

async function pickCamera() {
  if (disabled.value) return;
  await openCamera();
}

async function onFile(event) {
  const files = Array.from(event.target.files || []);
  for (const f of files) {
    await props.messenger.sendAttachment(f);
  }
  event.target.value = "";
}

function startHold() {
  if (disabled.value || recording.value) return;
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

async function insertEmoji(emoji) {
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
  // Respect the same 200-char cap the composer enforces via maxlength
  if (next.length > 200) next = next.slice(0, 200);
  props.messenger.state.messageInput = next;

  await nextTick();
  input.focus();
  const pos = Math.min(next.length, before.length + emoji.length);
  try { input.setSelectionRange(pos, pos); } catch { /* some input types throw */ }
}

function onDocMouseDown(event) {
  if (!pickerOpen.value) return;
  if (!emojiWrapRef.value) return;
  if (!emojiWrapRef.value.contains(event.target)) pickerOpen.value = false;
}

function onDocKey(event) {
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
  ctx.drawImage(video, 0, 0, width, height);

  const blob = await new Promise((resolve) => canvas.toBlob(resolve, "image/jpeg", 0.9));
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
});
onBeforeUnmount(() => {
  document.removeEventListener("mousedown", onDocMouseDown);
  document.removeEventListener("keydown", onDocKey);
  stopCameraStream();
});
</script>

<template>
  <footer class="composer">
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
      <div v-if="messenger.state.replyingTo" class="reply-draft">
        <div>
          <span class="reply-draft__label">Replying to {{ messenger.state.replyingTo.username || "message" }}</span>
          <span class="reply-draft__text">{{ messenger.state.replyingTo.text }}</span>
        </div>
        <button type="button" class="icon-btn" aria-label="Cancel reply" @click="messenger.cancelReply">
          <svg viewBox="0 0 24 24"><path d="M18 6 6 18M6 6l12 12"/></svg>
        </button>
      </div>
      <button class="icon-btn" type="button" aria-label="Attach file" :disabled="disabled" @click="pickFile">
        <svg viewBox="0 0 24 24"><path d="M21.44 11.05 12.25 20.24a6 6 0 0 1-8.49-8.49l9.19-9.19a4 4 0 1 1 5.66 5.66l-9.2 9.19a2 2 0 1 1-2.83-2.83L14.83 7"/></svg>
      </button>
      <button class="icon-btn" type="button" aria-label="Take photo" :disabled="disabled" @click="pickCamera">
        <svg viewBox="0 0 24 24"><path d="M4 7h3l1.4-2h7.2L17 7h3a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V9a2 2 0 0 1 2-2Z"/><circle cx="12" cy="13" r="3.5"/></svg>
      </button>

      <label class="composer__input">
        <input
          ref="inputRef"
          v-model="messenger.state.messageInput"
          maxlength="200"
          :placeholder="disabled ? 'Join a room to start messaging' : 'Message'"
          :disabled="disabled"
          autocomplete="off"
          spellcheck="false"
          @keydown.enter.exact.prevent="send"
        />
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
        :disabled="disabled"
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
