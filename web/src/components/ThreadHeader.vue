<script setup>
import { computed } from "vue";

const props = defineProps({
  messenger: { type: Object, required: true }
});

const name = computed(() => props.messenger.activeConversation.value?.name || props.messenger.roomLabel.value);
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
  const id = props.messenger.roomLabel.value;
  if (!id) return;
  if (!confirm(`Remove conversation "${id}" and its messages?`)) return;
  props.messenger.removeRoom(id);
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
        <svg v-if="!callActiveHere" viewBox="0 0 24 24"><path d="M22 16.92v3a2 2 0 0 1-2.18 2 19.86 19.86 0 0 1-8.63-3.07 19.5 19.5 0 0 1-6-6A19.86 19.86 0 0 1 2.12 4.18 2 2 0 0 1 4.11 2h3a2 2 0 0 1 2 1.72c.12.91.34 1.8.66 2.65a2 2 0 0 1-.45 2.11L8.09 9.91a16 16 0 0 0 6 6l1.43-1.23a2 2 0 0 1 2.11-.45c.85.32 1.74.54 2.65.66A2 2 0 0 1 22 16.92Z"/></svg>
        <svg v-else viewBox="0 0 24 24"><path d="M22 16.92v3a2 2 0 0 1-2.18 2 19.86 19.86 0 0 1-8.63-3.07 19.5 19.5 0 0 1-6-6A19.86 19.86 0 0 1 2.12 4.18 2 2 0 0 1 4.11 2h3a2 2 0 0 1 2 1.72c.12.91.34 1.8.66 2.65a2 2 0 0 1-.45 2.11L8.09 9.91a16 16 0 0 0 6 6l1.43-1.23a2 2 0 0 1 2.11-.45c.85.32 1.74.54 2.65.66A2 2 0 0 1 22 16.92Z" transform="rotate(135 12 12)"/></svg>
      </button>
      <button class="icon-btn" type="button" aria-label="Leave room" @click="removeHere">
        <svg viewBox="0 0 24 24"><path d="M9 12h12"/><path d="m17 8 4 4-4 4"/><path d="M9 4h-4a2 2 0 0 0-2 2v12a2 2 0 0 0 2 2h4"/></svg>
      </button>
    </div>
  </header>
</template>
