<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from "vue";

const props = defineProps({
  messenger: { type: Object, required: true }
});

const now = ref(Date.now());
let tickId = null;

onMounted(() => {
  tickId = setInterval(() => { now.value = Date.now(); }, 500);
});
onBeforeUnmount(() => {
  if (tickId) clearInterval(tickId);
});

const callRoom = computed(() => props.messenger.state.callRoom);

const members = computed(() => {
  const roomId = callRoom.value;
  if (!roomId) return [];
  const list = props.messenger.state.voiceMembersByRoom[roomId] || [];
  // Put self first
  const me = props.messenger.state.username;
  const sorted = [...list].sort((a, b) => {
    if (a === me) return -1;
    if (b === me) return 1;
    return a.localeCompare(b);
  });
  return sorted;
});

const remoteMembers = computed(() => members.value.filter((username) => !isSelf(username)));

const callTiles = computed(() => {
  const tiles = [];
  for (const username of members.value) {
    const self = isSelf(username);
    const media = mediaOf(username);
    const videoKinds = [];
    if (media.screen) videoKinds.push("screen");
    if (media.camera) videoKinds.push("camera");

    if (videoKinds.length) {
      for (const kind of videoKinds) {
        tiles.push({
          id: `${username}-${kind}`,
          username,
          kind,
          video: true,
          self,
          media,
          trackIndex: kind === "screen" && media.camera ? 0 : videoKinds.indexOf(kind)
        });
      }
    } else {
      tiles.push({
        id: `${username}-avatar`,
        username,
        kind: "audio",
        video: false,
        self,
        media,
        trackIndex: 0
      });
    }
  }
  return tiles;
});

const callGridClass = computed(() => {
  const count = callTiles.value.length;
  if (count <= 1) return "callpanel__stage--solo";
  if (count === 2) return "callpanel__stage--duo";
  if (count <= 4) return "callpanel__stage--grid";
  return "callpanel__stage--many";
});

const speakingSet = computed(() => {
  const roomId = callRoom.value;
  const table = props.messenger.state.speakingByRoom[roomId] || {};
  const cutoff = now.value - 1500;
  return new Set(Object.keys(table).filter((u) => table[u] >= cutoff));
});

function initialsOf(name) {
  const trimmed = String(name || "?").trim();
  if (!trimmed) return "?";
  const parts = trimmed.split(/[\s\-_]+/).slice(0, 2);
  if (parts.length === 2 && parts[1]) return (parts[0][0] + parts[1][0]).toUpperCase();
  return trimmed.slice(0, 2).toUpperCase();
}

function isSelf(username) {
  return String(username || "") === String(props.messenger.state.username || "");
}

function isSpeaking(username) {
  if (isSelf(username)) {
    // self speaking = call is live and mic not muted (heuristic)
    return props.messenger.state.inCall && !props.messenger.state.callMuted;
  }
  return speakingSet.value.has(username);
}

function callElapsed() {
  return props.messenger.formatDuration(props.messenger.state.callElapsed);
}

function volumeOf(username) {
  return props.messenger.callUserVolume(username);
}

function inputValue(event) {
  return event.target?.value ?? "";
}

function mediaOf(username) {
  if (isSelf(username)) return props.messenger.state.localCallMedia || {};
  return props.messenger.state.remoteCallMediaByUser[username] || {};
}

function hasVideo(username) {
  const media = mediaOf(username);
  return Boolean(media.camera || media.screen);
}

function tileLabel(tile) {
  if (tile.kind === "screen") return "Screen";
  if (tile.kind === "camera") return "Camera";
  return "Voice";
}

function bindLocalPreview(el, kind) {
  if (!el) return;
  const stream = props.messenger.localPreviewStream(kind);
  if (el.srcObject !== stream) el.srcObject = stream;
}

function bindRemoteVideo(el, username, trackIndex = 0) {
  if (!el) return;
  const stream = props.messenger.remoteVideoStream(username);
  const track = stream?.getVideoTracks?.()[trackIndex] || stream?.getVideoTracks?.()[0];
  if (!track) {
    el.srcObject = null;
    return;
  }
  const existingTrack = el.srcObject?.getVideoTracks?.()[0];
  if (existingTrack?.id !== track.id) el.srcObject = new MediaStream([track]);
}

function bindRemoteAudio(el, username) {
  if (!el) return;
  const stream = props.messenger.remoteCallStream(username);
  if (el.srcObject !== stream) el.srcObject = stream;
  el.volume = Math.max(0, Math.min(1, volumeOf(username) / 100));
  props.messenger.applyAudioOutput(el);
  el.play?.().catch?.(() => {});
}
</script>

<template>
  <section class="callpanel" v-if="messenger.state.inCall && messenger.state.callRoom === messenger.state.activeRoom">
    <header class="callpanel__head">
      <div class="callpanel__meta">
        <span class="callpanel__status">
          <span class="call-dot"></span>
          Live
        </span>
        <span class="callpanel__title">{{ messenger.displayRoomName(callRoom) }}</span>
        <span class="callpanel__time">{{ callElapsed() }}</span>
      </div>
      <div class="callpanel__actions">
        <button
          class="icon-btn"
          :class="{ 'icon-btn--danger': messenger.state.callMuted }"
          type="button"
          :aria-label="messenger.state.callMuted ? 'Unmute' : 'Mute'"
          @click="messenger.toggleMute"
        >
          <svg v-if="!messenger.state.callMuted" viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M12 2a3 3 0 0 0-3 3v7a3 3 0 0 0 6 0V5a3 3 0 0 0-3-3Z"/><path d="M19 10a7 7 0 0 1-14 0"/><line x1="12" y1="19" x2="12" y2="23"/></svg>
          <svg v-else viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><line x1="1" y1="1" x2="23" y2="23"/><path d="M9 9v3a3 3 0 0 0 5.12 2.12M15 9.34V5a3 3 0 0 0-5.94-.6"/><path d="M17 16.95A7 7 0 0 1 5 12v-2m14 0v2a7 7 0 0 1-.11 1.23"/><line x1="12" y1="19" x2="12" y2="23"/></svg>
        </button>
        <button
          class="icon-btn"
          :class="{ 'icon-btn--active': messenger.state.callCameraEnabled }"
          type="button"
          :aria-label="messenger.state.callCameraEnabled ? 'Stop camera' : 'Start camera'"
          @click="messenger.toggleCamera"
        >
          <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M15 10.5 20 7v10l-5-3.5V17a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V7a2 2 0 0 1 2-2h8a2 2 0 0 1 2 2v3.5Z"/></svg>
        </button>
        <button
          class="icon-btn"
          :class="{ 'icon-btn--active': messenger.state.callScreenEnabled }"
          type="button"
          :aria-label="messenger.state.callScreenEnabled ? 'Stop screen share' : 'Share screen'"
          @click="messenger.toggleScreenShare"
        >
          <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="4" width="18" height="13" rx="2"/><path d="M8 21h8"/><path d="M12 17v4"/><path d="m9 10 3-3 3 3"/><path d="M12 7v7"/></svg>
        </button>
        <button
          class="icon-btn icon-btn--danger"
          type="button"
          aria-label="End call"
          @click="messenger.endCall"
        >
          <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M6.6 15.4c3.3-2.1 7.5-2.1 10.8 0l1.45.92c.7.44.92 1.37.48 2.07l-1.15 1.84c-.44.7-1.37.92-2.07.48l-1.55-.97a4.95 4.95 0 0 0-5.12 0l-1.55.97c-.7.44-1.63.22-2.07-.48l-1.15-1.84c-.44-.7-.22-1.63.48-2.07l1.45-.92Z"/><path d="M6 8.5C9.7 6.2 14.3 6.2 18 8.5"/><path d="M3.5 5.2c5.2-3.4 11.8-3.4 17 0"/></svg>
        </button>
      </div>
    </header>

    <div class="callpanel__stage" :class="callGridClass">
      <div
        v-for="tile in callTiles"
        :key="tile.id"
        class="calltile"
        :class="{
          'is-speaking': isSpeaking(tile.username),
          'is-self': tile.self,
          'is-muted': tile.self && messenger.state.callMuted,
          'has-video': tile.video,
          'is-screen': tile.kind === 'screen'
        }"
      >
        <div v-if="tile.video" class="calltile__video">
          <video
            v-if="tile.self"
            :ref="(el) => bindLocalPreview(el, tile.kind)"
            autoplay
            muted
            playsinline
          ></video>
          <video
            v-else
            :ref="(el) => bindRemoteVideo(el, tile.username, tile.trackIndex)"
            autoplay
            playsinline
          ></video>
        </div>
        <div v-else class="calltile__empty">
          <span
            class="calltile__avatar"
            :class="`avatar--${messenger.accentFor(tile.username)}`"
          >{{ initialsOf(tile.username) }}</span>
        </div>
        <div class="calltile__overlay">
          <span class="calltile__name">
            {{ tile.username }}<span v-if="tile.self" class="calltile__you"> (you)</span>
          </span>
          <span class="calltile__kind">{{ tileLabel(tile) }}</span>
          <span v-if="tile.self && messenger.state.callMuted" class="calltile__muted-badge" aria-label="muted">
            <svg viewBox="0 0 24 24" width="12" height="12" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><line x1="1" y1="1" x2="23" y2="23"/><path d="M9 9v3a3 3 0 0 0 5.12 2.12"/><path d="M15 9.34V5a3 3 0 0 0-5.94-.6"/></svg>
          </span>
        </div>
      </div>
    </div>

    <div class="callpanel__audio" v-if="remoteMembers.length">
      <label v-for="u in remoteMembers" :key="`audio-${u}`" class="calltile__volume" :aria-label="`${u} volume`">
        <audio
          :ref="(el) => bindRemoteAudio(el, u)"
          autoplay
          playsinline
        ></audio>
        <span class="calltile__volume-name">{{ u }}</span>
        <svg viewBox="0 0 24 24"><path d="M11 5 6 9H3v6h3l5 4V5Z"/><path d="M15.5 8.5a5 5 0 0 1 0 7"/><path d="M18.5 5.5a9 9 0 0 1 0 13"/></svg>
        <input
          type="range"
          min="0"
          max="100"
          step="1"
          :value="volumeOf(u)"
          @input="messenger.setCallUserVolume(u, inputValue($event))"
        />
        <span>{{ volumeOf(u) }}%</span>
      </label>
    </div>
  </section>
</template>
