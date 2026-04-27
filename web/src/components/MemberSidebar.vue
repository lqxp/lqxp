<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import ProfileCard from "@/components/ProfileCard.vue";

const props = defineProps({
  messenger: { type: Object, required: true }
});

function initialsFor(name: string) {
  const clean = String(name || "?").trim();
  const parts = clean.split(/[\s\-_]+/).filter(Boolean).slice(0, 2);
  if (parts.length >= 2) return (parts[0][0] + parts[1][0]).toUpperCase();
  return clean.slice(0, 2).toUpperCase() || "?";
}

function accentFor(name: string) {
  const palette = ["blue", "green", "amber", "violet", "olive", "slate", "teal", "rose"];
  let hash = 0;
  for (const char of String(name || "")) hash = (hash * 31 + char.charCodeAt(0)) >>> 0;
  return palette[hash % palette.length];
}

function avatarFor(username: string) {
  return props.messenger.profileImageSrc(props.messenger.profileFor(username).avatar);
}

function statusFor(username: string) {
  return props.messenger.statusFor(username);
}

function statusLabel(username: string) {
  return props.messenger.presenceStatusLabel(statusFor(username));
}

const members = computed(() =>
  [...(props.messenger.memberRoster.value || [])].sort((a, b) => a.localeCompare(b))
);

const me = computed(() => String(props.messenger.state.username || "").trim());
const voiceMembers = computed(() => new Set(props.messenger.state.voiceMembersByRoom[props.messenger.state.activeRoom] || []));
const selectedProfile = ref("");

const sections = computed(() => {
  const self: string[] = [];
  const inCall: string[] = [];
  const online: string[] = [];

  for (const username of members.value) {
    if (username === me.value) {
      self.push(username);
      continue;
    }
    if (voiceMembers.value.has(username)) {
      inCall.push(username);
      continue;
    }
    online.push(username);
  }

  return [
    { key: "self", label: "You", users: self },
    { key: "call", label: "In call", users: inCall },
    { key: "online", label: "Online", users: online }
  ].filter((section) => section.users.length);
});

function openProfile(event: MouseEvent, username: string) {
  event.preventDefault();
  event.stopPropagation();
  selectedProfile.value = username;
}

function closeProfile() {
  selectedProfile.value = "";
}

function onKey(event: KeyboardEvent) {
  if (event.key === "Escape") closeProfile();
}

onMounted(() => document.addEventListener("keydown", onKey));
onBeforeUnmount(() => document.removeEventListener("keydown", onKey));
</script>

<template>
  <aside class="members" aria-label="Online members">
    <div class="members__head">
      <div>
        <div class="members__eyebrow">Presence</div>
        <div class="members__title">{{ members.length }} online</div>
      </div>
    </div>

    <div v-if="sections.length" class="members__sections">
      <section v-for="section in sections" :key="section.key" class="members__group">
        <div class="members__label">{{ section.label }} — {{ section.users.length }}</div>
        <div class="members__list">
          <div
            v-for="username in section.users"
            :key="username"
            class="members__item"
            role="button"
            tabindex="0"
            :aria-label="`Open profile: ${username}`"
            @click="openProfile($event, username)"
            @contextmenu.prevent.stop="openProfile($event, username)"
            @keydown.enter.prevent="selectedProfile = username"
            @keydown.space.prevent="selectedProfile = username"
          >
            <span v-if="avatarFor(username)" class="members__avatar-image">
              <img :src="avatarFor(username)" alt="" />
            </span>
            <span v-else class="avatar avatar--sm" :class="`avatar--${accentFor(username)}`">
              {{ initialsFor(username) }}
            </span>
            <div class="members__meta">
              <div class="members__name">
                {{ username }}
                <span v-if="username === me" class="members__badge">you</span>
              </div>
              <div class="members__status">
                <span
                  class="members__dot"
                  :class="{
                    'is-call': voiceMembers.has(username),
                    'is-dnd': statusFor(username) === 'dnd',
                    'is-invisible': statusFor(username) === 'invisible'
                  }"
                ></span>
                {{ voiceMembers.has(username) ? "In voice chat" : statusLabel(username) }}
              </div>
            </div>
          </div>
        </div>
      </section>
    </div>

    <div v-else class="members__empty">No one is online in this room yet.</div>

    <Teleport to="body">
      <ProfileCard
        v-if="selectedProfile"
        :messenger="messenger"
        :username="selectedProfile"
        @close="closeProfile"
      />
    </Teleport>
  </aside>
</template>
