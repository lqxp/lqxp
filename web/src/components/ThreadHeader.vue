<script setup>
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

const subtitle = computed(() => {
  const members = props.messenger.memberRoster.value || [];
  if (!members.length) return "";
  if (members.length === 1) return `${members[0]} online`;
  return `${members.length} online · ${members.slice(0, 3).join(", ")}${members.length > 3 ? " …" : ""}`;
});

const callActiveHere = computed(() =>
  props.messenger.state.inCall && props.messenger.state.callRoom === props.messenger.state.activeRoom
);

const callElapsed = computed(() => props.messenger.formatDuration(props.messenger.state.callElapsed));

function toggleCall() {
  if (callActiveHere.value) props.messenger.endCall();
  else props.messenger.startCall();
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
          <template v-else>{{ subtitle }}</template>
        </div>
      </div>
    </div>

    <div class="thread__tools">
      <button
        class="icon-btn"
        :class="{ 'icon-btn--active': callActiveHere, 'icon-btn--danger': callActiveHere }"
        type="button"
        :aria-label="callActiveHere ? 'End call' : 'Start call'"
        @click="toggleCall"
      >
        <svg v-if="!callActiveHere" viewBox="0 0 24 24"><path d="M7.6 10.8a14.5 14.5 0 0 0 5.6 5.6l1.9-1.9a1.5 1.5 0 0 1 1.5-.37c1.03.34 2.1.52 3.2.52.83 0 1.5.67 1.5 1.5v3.05c0 .83-.67 1.5-1.5 1.5C10.45 20.7 3.3 13.55 3.3 4.2c0-.83.67-1.5 1.5-1.5h3.05c.83 0 1.5.67 1.5 1.5 0 1.1.18 2.17.52 3.2.17.53.03 1.1-.37 1.5l-1.9 1.9Z"/></svg>
        <svg v-else viewBox="0 0 24 24"><path d="M6.6 15.4c3.3-2.1 7.5-2.1 10.8 0l1.45.92c.7.44.92 1.37.48 2.07l-1.15 1.84c-.44.7-1.37.92-2.07.48l-1.55-.97a4.95 4.95 0 0 0-5.12 0l-1.55.97c-.7.44-1.63.22-2.07-.48l-1.15-1.84c-.44-.7-.22-1.63.48-2.07l1.45-.92Z"/><path d="M6 8.5C9.7 6.2 14.3 6.2 18 8.5"/><path d="M3.5 5.2c5.2-3.4 11.8-3.4 17 0"/></svg>
      </button>
      <button class="icon-btn" type="button" aria-label="Leave room" @click="removeHere">
        <svg viewBox="0 0 24 24"><path d="M9 12h12"/><path d="m17 8 4 4-4 4"/><path d="M9 4h-4a2 2 0 0 0-2 2v12a2 2 0 0 0 2 2h4"/></svg>
      </button>
    </div>
  </header>
</template>
