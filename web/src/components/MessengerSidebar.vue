<script setup>
import { computed, nextTick, ref, watch } from "vue";

const props = defineProps({
  messenger: { type: Object, required: true }
});

const composeRef = ref(null);

const meInitials = computed(() => initialsOf(props.messenger.state.username));
const meAccent = computed(() => props.messenger.accentFor(props.messenger.state.username || "you"));

function initialsOf(name) {
  const trimmed = String(name || "?").trim();
  if (!trimmed) return "?";
  const parts = trimmed.split(/[\s\-_]+/).slice(0, 2);
  if (parts.length === 2 && parts[1]) return (parts[0][0] + parts[1][0]).toUpperCase();
  return trimmed.slice(0, 2).toUpperCase();
}

watch(
  () => props.messenger.state.composing,
  async (isComposing) => {
    if (isComposing) { await nextTick(); composeRef.value?.focus(); }
  }
);

function onComposeKey(event) {
  if (event.key === "Escape") props.messenger.cancelCompose();
  if (event.key.length === 1 && !/[a-z0-9]/i.test(event.key)) event.preventDefault();
}

function removeConversation(event, roomId) {
  event.stopPropagation();
  event.preventDefault();
  if (!confirm(`Remove conversation "${props.messenger.displayRoomName(roomId)}" and its messages?`)) return;
  props.messenger.removeRoom(roomId);
}

function openSettings() {
  props.messenger.state.settingsOpen = true;
}
</script>

<template>
  <aside class="side">
    <div class="side__top">
      <button class="side__me" type="button" @click="openSettings" :title="messenger.state.username">
        <span class="avatar avatar--md" :class="`avatar--${meAccent}`">{{ meInitials }}</span>
        <span class="side__me-name">{{ messenger.state.username || "anonymous" }}</span>
      </button>

      <div class="side__actions">
        <button class="icon-btn" type="button" aria-label="New conversation" @click="messenger.startCompose">
          <svg viewBox="0 0 24 24"><path d="M12 20h9"/><path d="M16.5 3.5a2.1 2.1 0 1 1 3 3L7 19l-4 1 1-4Z"/></svg>
        </button>
        <button class="icon-btn" type="button" aria-label="Shuffle playlist room" title="Shuffle playlist room" @click="messenger.createRandomRoom">
          <svg viewBox="0 0 24 24">
            <path d="M3 7h3.6c1.5 0 2.7.8 3.5 2.1l3.8 5.8c.8 1.3 2 2.1 3.5 2.1H21"/>
            <path d="M18 14l3 3-3 3"/>
            <path d="M3 17h3.6c1.5 0 2.7-.8 3.5-2.1"/>
            <path d="M13.9 9.1c.8-1.3 2-2.1 3.5-2.1H21"/>
            <path d="M18 4l3 3-3 3"/>
          </svg>
        </button>
        <button class="icon-btn" type="button" aria-label="Settings" @click="openSettings">
          <svg viewBox="0 0 24 24"><circle cx="12" cy="5" r="1.2"/><circle cx="12" cy="12" r="1.2"/><circle cx="12" cy="19" r="1.2"/></svg>
        </button>
      </div>
    </div>

    <div v-if="messenger.state.composing" class="compose">
      <input
        ref="composeRef"
        v-model="messenger.state.composeInput"
        type="text"
        maxlength="64"
        minlength="8"
        pattern="[A-Za-z0-9]{8,64}"
        autocomplete="off"
        spellcheck="false"
        placeholder="Room ID, 8-64 letters/numbers"
        @keydown.enter.prevent="messenger.submitCompose"
        @keydown="onComposeKey"
        @blur="messenger.state.composeInput ? null : messenger.cancelCompose()"
      />
      <button type="button" aria-label="Cancel" @click="messenger.cancelCompose">
        <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M18 6 6 18M6 6l12 12"/></svg>
      </button>
    </div>

    <div class="side__search" v-if="!messenger.state.composing">
      <label class="search">
        <svg viewBox="0 0 24 24"><circle cx="11" cy="11" r="7"/><path d="m20 20-3.5-3.5"/></svg>
        <input
          v-model="messenger.state.searchTerm"
          type="search"
          placeholder="Search"
          aria-label="Search conversations"
        />
      </label>
    </div>

    <div class="side__list">
      <template v-if="messenger.conversations.value.length">
        <div
          v-for="c in messenger.conversations.value"
          :key="c.roomId"
          class="conv"
          :class="{ 'is-active': c.active }"
          role="button"
          tabindex="0"
          @click="messenger.selectConversation(c.roomId)"
          @keydown.enter.prevent="messenger.selectConversation(c.roomId)"
        >
          <span class="avatar avatar--lg" :class="`avatar--${c.accent}`">
            {{ initialsOf(c.name) }}
          </span>

          <span class="conv__head">
            <span class="conv__name">
              {{ c.name }}
              <span v-if="c.joined" class="conv__joined" title="Joined"></span>
            </span>
            <span class="conv__time">{{ c.timestampLabel }}</span>
          </span>

          <span class="conv__preview">
            {{ c.preview }}
          </span>

          <span v-if="c.unread > 0" class="conv__badge">{{ c.unread > 99 ? "99+" : c.unread }}</span>

          <button
            class="conv__remove"
            type="button"
            :aria-label="`Remove ${c.name}`"
            @click="removeConversation($event, c.roomId)"
          >
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M18 6 6 18M6 6l12 12"/></svg>
          </button>
        </div>
      </template>
      <div v-else class="conv--empty">
        No conversations yet.<br/>
        Tap the pencil icon to start one.
      </div>
    </div>

    <div class="side__foot">
      <span class="dot" :class="{
        'is-online': messenger.state.connected && messenger.state.identified,
        'is-connecting': messenger.state.connected && !messenger.state.identified
      }"></span>
      <span v-if="messenger.state.connected && messenger.state.identified">
        connected · {{ messenger.state.joinedRooms.length }} open
      </span>
      <span v-else-if="messenger.state.connected">authenticating…</span>
      <span v-else>offline</span>
      <span class="spacer"></span>
      <button
        v-if="!messenger.state.connected"
        class="btn--ghost"
        type="button"
        @click="messenger.connect"
      >connect</button>
      <button
        v-else
        class="btn--ghost"
        type="button"
        @click="messenger.disconnect"
      >disconnect</button>
    </div>
  </aside>
</template>
