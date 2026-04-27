<script setup lang="ts">
import { computed } from "vue";

const props = defineProps({
  messenger: { type: Object, required: true },
  username: { type: String, required: true }
});

const emit = defineEmits(["close"]);

const profile = computed(() => props.messenger.profileFor(props.username));
const avatarSrc = computed(() => props.messenger.profileImageSrc(profile.value.avatar));
const bannerSrc = computed(() => props.messenger.profileImageSrc(profile.value.banner));
const accent = computed(() => props.messenger.accentFor(props.username));
const isSelf = computed(() => String(props.messenger.state.username || "").trim() === props.username);
const voiceMembers = computed(() => new Set(props.messenger.state.voiceMembersByRoom[props.messenger.state.activeRoom] || []));
const statusLabel = computed(() => voiceMembers.value.has(props.username) ? "In voice chat" : "Online");

function initialsFor(name: string) {
  const clean = String(name || "?").trim();
  const parts = clean.split(/[\s\-_]+/).filter(Boolean).slice(0, 2);
  if (parts.length >= 2) return (parts[0][0] + parts[1][0]).toUpperCase();
  return clean.slice(0, 2).toUpperCase() || "?";
}
</script>

<template>
  <div class="profile-card" role="dialog" aria-modal="true" :aria-label="`Profile: ${username}`" @click="emit('close')">
    <section class="profile-card__panel" @click.stop>
      <div class="profile-card__banner" :class="{ 'has-image': bannerSrc }">
        <img v-if="bannerSrc" :src="bannerSrc" alt="" />
      </div>
      <div class="profile-card__body">
        <span v-if="avatarSrc" class="profile-card__avatar profile-card__avatar--image">
          <img :src="avatarSrc" alt="" />
        </span>
        <span v-else class="avatar profile-card__avatar" :class="`avatar--${accent}`">{{ initialsFor(username) }}</span>
        <button class="icon-btn profile-card__close" type="button" aria-label="Close profile" @click="emit('close')">
          <svg viewBox="0 0 24 24"><path d="M18 6 6 18M6 6l12 12"/></svg>
        </button>
        <div class="profile-card__identity">
          <strong>{{ username }}</strong>
          <small>
            <span class="members__dot" :class="{ 'is-call': voiceMembers.has(username) }"></span>
            {{ statusLabel }}<template v-if="profile.pronouns"> · {{ profile.pronouns }}</template><template v-if="isSelf"> · you</template>
          </small>
        </div>
        <div class="profile-card__section">
          <h4>About</h4>
          <p v-if="profile.description" class="profile-card__description">{{ profile.description }}</p>
          <p v-else class="profile-card__empty">No profile description.</p>
        </div>
      </div>
    </section>
  </div>
</template>
