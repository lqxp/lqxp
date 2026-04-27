<script setup lang="ts">
import { computed } from "vue";

const props = defineProps({
  messenger: { type: Object, required: true }
});

const name = computed(() => props.messenger.displayRoomName(props.messenger.state.activeRoom));
const accent = computed(() => props.messenger.activeConversation.value?.accent || "slate");

const initials = computed(() => {
  const n = String(name.value || "?").trim();
  const parts = n.split(/[\s\-_]+/).slice(0, 2);
  if (parts.length === 2 && parts[1]) return (parts[0][0] + parts[1][0]).toUpperCase();
  return n.slice(0, 2).toUpperCase() || "?";
});

const callActiveHere = computed(() =>
  props.messenger.state.inCall && props.messenger.state.callRoom === props.messenger.state.activeRoom
);

const callElapsed = computed(() => props.messenger.formatDuration(props.messenger.state.callElapsed));
const roomHasKey = computed(() => props.messenger.hasRoomKey(props.messenger.state.activeRoom));
const securityLabel = computed(() => roomHasKey.value ? "E2EE ready" : "No room key yet");

function startCall() {
  props.messenger.startCall();
}

function copyInvite() {
  const id = props.messenger.state.activeRoom;
  if (!id) return;
  props.messenger.copyRoomInvite(id)
    .then(() => {
      props.messenger.showToast("Encrypted invite link copied.");
    })
    .catch((error) => {
      props.messenger.state.lastError = error?.message || "Could not copy invite link.";
    });
}

function removeHere() {
  const id = props.messenger.state.activeRoom;
  if (!id) return;
  const label = props.messenger.displayRoomName(id);
  const suffix = props.messenger.state.deleteMessagesOnLeave ? " Local messages will be deleted." : "";
  if (!confirm(`Leave "${label}"?${suffix}`)) return;
  props.messenger.leaveRoom(id);
}
</script>

<template>
  <header class="thread__head">
    <div class="thread__who">
      <span class="avatar avatar--md" :class="`avatar--${accent}`">{{ initials }}</span>
      <div>
        <div class="thread__name">{{ name }}</div>
        <div class="thread__sub">
          <template v-if="callActiveHere">
            <span class="call-dot"></span>
            In call · {{ callElapsed }}
          </template>
          <template v-else>Room conversation · {{ securityLabel }}</template>
        </div>
      </div>
    </div>

    <div class="thread__tools">
      <button
        class="icon-btn"
        type="button"
        aria-label="Copy encrypted invite link"
        @click="copyInvite"
      >
        <svg viewBox="0 0 24 24"><path d="M15 8a3 3 0 0 1 3 3v6a3 3 0 0 1-3 3H8a3 3 0 0 1-3-3v-6a3 3 0 0 1 3-3"/><path d="M9 8V6a3 3 0 1 1 6 0v2"/><rect x="8" y="8" width="8" height="6" rx="1.5"/></svg>
      </button>
      <button
        v-if="!callActiveHere"
        class="icon-btn"
        type="button"
        aria-label="Start call"
        @click="startCall"
      >
        <svg viewBox="0 0 24 24"><path d="M7.6 10.8a14.5 14.5 0 0 0 5.6 5.6l1.9-1.9a1.5 1.5 0 0 1 1.5-.37c1.03.34 2.1.52 3.2.52.83 0 1.5.67 1.5 1.5v3.05c0 .83-.67 1.5-1.5 1.5C10.45 20.7 3.3 13.55 3.3 4.2c0-.83.67-1.5 1.5-1.5h3.05c.83 0 1.5.67 1.5 1.5 0 1.1.18 2.17.52 3.2.17.53.03 1.1-.37 1.5l-1.9 1.9Z"/></svg>
      </button>
      <button class="icon-btn" type="button" aria-label="Leave room" @click="removeHere">
        <svg viewBox="0 0 24 24"><path d="M9 12h12"/><path d="m17 8 4 4-4 4"/><path d="M9 4h-4a2 2 0 0 0-2 2v12a2 2 0 0 0 2 2h4"/></svg>
      </button>
    </div>
  </header>
</template>
