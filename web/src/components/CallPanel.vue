<script setup>
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
</script>

<template>
  <section class="callpanel" v-if="messenger.state.inCall && messenger.state.callRoom === messenger.state.activeRoom">
    <header class="callpanel__head">
      <div class="callpanel__meta">
        <span class="call-dot"></span>
        <span class="callpanel__title">Voice — {{ callRoom }}</span>
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
          class="icon-btn icon-btn--danger"
          type="button"
          aria-label="End call"
          @click="messenger.endCall"
        >
          <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
            <path d="M22 16.92v3a2 2 0 0 1-2.18 2 19.86 19.86 0 0 1-8.63-3.07 19.5 19.5 0 0 1-6-6A19.86 19.86 0 0 1 2.12 4.18 2 2 0 0 1 4.11 2h3a2 2 0 0 1 2 1.72c.12.91.34 1.8.66 2.65a2 2 0 0 1-.45 2.11L8.09 9.91a16 16 0 0 0 6 6l1.43-1.23a2 2 0 0 1 2.11-.45c.85.32 1.74.54 2.65.66A2 2 0 0 1 22 16.92Z" transform="rotate(135 12 12)"/>
          </svg>
        </button>
      </div>
    </header>

    <div class="callpanel__tiles">
      <div
        v-for="u in members"
        :key="`call-${u}`"
        class="calltile"
        :class="{
          'is-speaking': isSpeaking(u),
          'is-self': isSelf(u),
          'is-muted': isSelf(u) && messenger.state.callMuted
        }"
      >
        <span
          class="calltile__avatar"
          :class="`avatar--${messenger.accentFor(u)}`"
        >{{ initialsOf(u) }}</span>
        <span class="calltile__name">
          {{ u }}<span v-if="isSelf(u)" class="calltile__you"> (you)</span>
        </span>
        <span v-if="isSelf(u) && messenger.state.callMuted" class="calltile__muted-badge" aria-label="muted">
          <svg viewBox="0 0 24 24" width="12" height="12" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><line x1="1" y1="1" x2="23" y2="23"/><path d="M9 9v3a3 3 0 0 0 5.12 2.12"/><path d="M15 9.34V5a3 3 0 0 0-5.94-.6"/></svg>
        </span>
      </div>
    </div>
  </section>
</template>
