<script setup lang="ts">
import { computed, nextTick, onMounted, onBeforeUnmount, ref, watch } from "vue";

const props = defineProps({
  messenger: { type: Object, required: true }
});

const draftName = ref(props.messenger.state.username || "");
const draftDescription = ref(props.messenger.state.profile?.description || "");
const draftPronouns = ref(props.messenger.state.profile?.pronouns || "");
const fileInputRef = ref(null);
const avatarInputRef = ref(null);
const bannerInputRef = ref(null);
const firstInputRef = ref(null);
const activeSection = ref("profile");

const isOpen = computed(() => props.messenger.state.settingsOpen);

const nameChanged = computed(() => draftName.value.trim() !== String(props.messenger.state.username || "").trim());
const nameValid = computed(() => !props.messenger.validateUsername(draftName.value));
const meAccent = computed(() => props.messenger.accentFor(props.messenger.state.username || "you"));
const meInitials = computed(() => initialsOf(props.messenger.state.username));
const profile = computed(() => props.messenger.myProfile.value);
const avatarSrc = computed(() => props.messenger.profileImageSrc(profile.value.avatar));
const bannerSrc = computed(() => props.messenger.profileImageSrc(profile.value.banner));
const profileTextChanged = computed(() =>
  draftDescription.value.trim() !== String(profile.value.description || "").trim()
  || draftPronouns.value.trim() !== String(profile.value.pronouns || "").trim()
);

const allSections = [
  { id: "profile", label: "Profile" },
  { id: "security", label: "Security" },
  { id: "privacy", label: "Privacy" },
  { id: "notifications", label: "Notifications" },
  { id: "calls", label: "Calls" },
  { id: "admin", label: "Admin" },
  { id: "backups", label: "Backups" },
  { id: "about", label: "About" }
];
const sections = computed(() => allSections.filter((section) => section.id !== "admin" || props.messenger.state.admin));

watch(isOpen, async (v) => {
  if (v) {
    draftName.value = props.messenger.state.username || "";
    draftDescription.value = props.messenger.state.profile?.description || "";
    draftPronouns.value = props.messenger.state.profile?.pronouns || "";
    props.messenger.refreshAudioDevices();
    await nextTick();
    if (activeSection.value === "profile") {
      firstInputRef.value?.focus();
      firstInputRef.value?.select();
    }
  }
});

watch(activeSection, async (section) => {
  if (!isOpen.value) return;
  if (section === "calls") props.messenger.refreshAudioDevices();
  if (section === "admin") props.messenger.loadAdminOverview();
  if (section !== "calls") props.messenger.stopMicTest();
  if (section === "profile") {
    await nextTick();
    firstInputRef.value?.focus();
  }
});

function close() {
  props.messenger.stopMicTest();
  props.messenger.state.settingsOpen = false;
}

async function saveName() {
  if (!nameValid.value || !nameChanged.value) return;
  await props.messenger.changeUsername(draftName.value.trim());
  draftName.value = props.messenger.state.username || "";
}

function saveProfileText() {
  if (!profileTextChanged.value) return;
  props.messenger.setProfileText({
    description: draftDescription.value,
    pronouns: draftPronouns.value
  });
}

function onAvatarPicked(event) {
  const file = event.target.files?.[0];
  if (file) props.messenger.setProfileImageFromFile("avatar", file);
  event.target.value = "";
}

function onBannerPicked(event) {
  const file = event.target.files?.[0];
  if (file) props.messenger.setProfileImageFromFile("banner", file);
  event.target.value = "";
}

function onExport() { props.messenger.exportData(); }
function onImport() { fileInputRef.value?.click(); }
function onFilePicked(event) {
  const file = event.target.files?.[0];
  if (file) props.messenger.importData(file);
  event.target.value = "";
}
function onClear() {
  if (!confirm("Clear all local data? This removes every conversation, message, and reaction from this browser. The remote server is not touched.")) return;
  props.messenger.clearAllData();
  close();
}

function onLogout() {
  props.messenger.logoutAccount();
  close();
}

function targetChecked(event: Event) {
  return Boolean((event.target as HTMLInputElement | null)?.checked);
}

function targetValue(event: Event) {
  return (event.target as HTMLInputElement | HTMLSelectElement | null)?.value || "";
}

const microphones = computed(() =>
  props.messenger.state.audioDevices.filter((device) => device.kind === "audioinput")
);
const headphones = computed(() =>
  props.messenger.state.audioDevices.filter((device) => device.kind === "audiooutput")
);

function deviceLabel(device, fallback) {
  return device.label || fallback;
}

function initialsOf(name) {
  const trimmed = String(name || "?").trim();
  if (!trimmed) return "?";
  const parts = trimmed.split(/[\s\-_]+/).slice(0, 2);
  if (parts.length === 2 && parts[1]) return (parts[0][0] + parts[1][0]).toUpperCase();
  return trimmed.slice(0, 2).toUpperCase();
}

function onKey(event) {
  if (!isOpen.value) return;
  if (event.key === "Escape") close();
}

onMounted(() => document.addEventListener("keydown", onKey));
onBeforeUnmount(() => document.removeEventListener("keydown", onKey));
</script>

<template>
  <div v-if="isOpen" class="settings" role="dialog" aria-modal="true" aria-labelledby="settings-title">
    <aside class="settings__side">
      <header class="settings__side-head">
        <h2 id="settings-title">Settings</h2>
        <button class="icon-btn settings__close" type="button" aria-label="Close settings" @click="close">
          <svg viewBox="0 0 24 24"><path d="M18 6 6 18M6 6l12 12"/></svg>
        </button>
      </header>

      <button class="settings__card" type="button" @click="activeSection = 'profile'">
        <span class="avatar avatar--md" :class="`avatar--${meAccent}`">{{ meInitials }}</span>
        <span>
          <strong>{{ messenger.state.username || "anonymous" }}</strong>
          <small>{{ messenger.connectionLabel.value }}</small>
        </span>
      </button>

      <nav class="settings__nav" aria-label="Settings sections">
        <button
          v-for="section in sections"
          :key="section.id"
          type="button"
          class="settings__nav-item"
          :class="{ 'is-active': activeSection === section.id }"
          @click="activeSection = section.id"
        >
          <svg v-if="section.id === 'profile'" viewBox="0 0 24 24"><circle cx="12" cy="8" r="4"/><path d="M4 21a8 8 0 0 1 16 0"/></svg>
          <svg v-else-if="section.id === 'privacy'" viewBox="0 0 24 24"><path d="M12 3 5 6v5c0 4.5 2.9 8.5 7 10 4.1-1.5 7-5.5 7-10V6l-7-3Z"/><path d="M9.5 12.5 11 14l3.5-4"/></svg>
          <svg v-else-if="section.id === 'notifications'" viewBox="0 0 24 24"><path d="M18 8a6 6 0 0 0-12 0c0 7-3 7-3 9h18c0-2-3-2-3-9Z"/><path d="M10 21h4"/></svg>
          <svg v-else-if="section.id === 'calls'" viewBox="0 0 24 24"><path d="M7.6 10.8a14.5 14.5 0 0 0 5.6 5.6l1.9-1.9a1.5 1.5 0 0 1 1.5-.37c1.03.34 2.1.52 3.2.52.83 0 1.5.67 1.5 1.5v3.05c0 .83-.67 1.5-1.5 1.5C10.45 20.7 3.3 13.55 3.3 4.2c0-.83.67-1.5 1.5-1.5h3.05c.83 0 1.5.67 1.5 1.5 0 1.1.18 2.17.52 3.2.17.53.03 1.1-.37 1.5l-1.9 1.9Z"/></svg>
          <svg v-else-if="section.id === 'backups'" viewBox="0 0 24 24"><path d="M12 3v12"/><path d="m6 9 6-6 6 6"/><path d="M5 21h14"/></svg>
          <svg v-else viewBox="0 0 24 24"><circle cx="12" cy="12" r="10"/><path d="M12 16v-4"/><path d="M12 8h.01"/></svg>
          <span>{{ section.label }}</span>
        </button>
      </nav>
    </aside>

    <main class="settings__main">
      <header class="settings__main-head">
        <h3>{{ sections.find((section) => section.id === activeSection)?.label }}</h3>
      </header>

      <section v-if="activeSection === 'profile'" class="settings-page">
        <div class="settings-profile">
          <div class="settings-profile__banner" :class="{ 'has-image': bannerSrc }">
            <img v-if="bannerSrc" :src="bannerSrc" alt="" />
          </div>
          <span v-if="avatarSrc" class="settings-profile__avatar-image">
            <img :src="avatarSrc" alt="" />
          </span>
          <span v-else class="avatar settings-profile__avatar" :class="`avatar--${meAccent}`">{{ meInitials }}</span>
          <div class="settings-profile__actions">
            <button type="button" class="btn settings-profile__photo" @click="avatarInputRef?.click()">Profile image</button>
            <button type="button" class="btn settings-profile__photo" @click="bannerInputRef?.click()">Banner</button>
            <button v-if="profile.avatar" type="button" class="btn settings-profile__photo" @click="messenger.clearProfileImage('avatar')">Clear image</button>
            <button v-if="profile.banner" type="button" class="btn settings-profile__photo" @click="messenger.clearProfileImage('banner')">Clear banner</button>
          </div>
          <input
            ref="avatarInputRef"
            type="file"
            accept="image/png,image/apng,image/gif,image/jpeg,.apng"
            style="display: none"
            @change="onAvatarPicked"
          />
          <input
            ref="bannerInputRef"
            type="file"
            accept="image/png,image/apng,image/gif,image/jpeg,.apng"
            style="display: none"
            @change="onBannerPicked"
          />
        </div>

        <div class="settings-group">
          <label class="settings-field">
            <span class="settings-field__icon">
              <svg viewBox="0 0 24 24"><circle cx="12" cy="8" r="4"/><path d="M4 21a8 8 0 0 1 16 0"/></svg>
            </span>
            <span class="settings-field__body">
              <span class="settings-field__label">Display name</span>
              <span class="settings-field__hint">Visible to people you message.</span>
            </span>
          </label>
          <div class="settings-inline">
            <input
              ref="firstInputRef"
              v-model="draftName"
              type="text"
              maxlength="32"
              autocomplete="off"
              spellcheck="false"
              placeholder="e.g. echo"
              class="settings-input"
              @keydown.enter.prevent="saveName"
            />
            <button
              type="button"
              class="btn btn--primary settings-btn"
              :disabled="!nameValid || !nameChanged"
              @click="saveName"
            >Save</button>
          </div>
        </div>

        <div class="settings-group">
          <label class="settings-field">
            <span class="settings-field__icon">
              <svg viewBox="0 0 24 24"><path d="M12 2v4"/><path d="M12 18v4"/><path d="m4.93 4.93 2.83 2.83"/><path d="m16.24 16.24 2.83 2.83"/><path d="M2 12h4"/><path d="M18 12h4"/><path d="m4.93 19.07 2.83-2.83"/><path d="m16.24 7.76 2.83-2.83"/></svg>
            </span>
            <span class="settings-field__body">
              <span class="settings-field__label">Status</span>
              <span class="settings-field__hint">Controls how you appear in Presence.</span>
            </span>
          </label>
          <label class="settings-select settings-select--offset">
            <span class="sr-only">Status</span>
            <select :value="messenger.state.status" @change="messenger.setPresenceStatus(targetValue($event))">
              <option value="online">Online</option>
              <option value="invisible">Invisible</option>
              <option value="dnd">Do Not Disturb</option>
            </select>
          </label>
        </div>

        <div class="settings-group">
          <label class="settings-field">
            <span class="settings-field__icon">
              <svg viewBox="0 0 24 24"><path d="M4 6h16"/><path d="M4 12h13"/><path d="M4 18h9"/></svg>
            </span>
            <span class="settings-field__body">
              <span class="settings-field__label">Description</span>
              <span class="settings-field__hint">{{ draftDescription.length }}/{{ messenger.MAX_PROFILE_DESCRIPTION_LENGTH }}</span>
            </span>
          </label>
          <textarea
            v-model="draftDescription"
            class="settings-input settings-textarea"
            :maxlength="messenger.MAX_PROFILE_DESCRIPTION_LENGTH"
            spellcheck="true"
            rows="4"
            placeholder="Write a short profile description"
          ></textarea>
        </div>

        <div class="settings-group">
          <label class="settings-field">
            <span class="settings-field__icon">
              <svg viewBox="0 0 24 24"><path d="M5 7h14"/><path d="M8 7v10"/><path d="M16 7v10"/><path d="M4 17h16"/></svg>
            </span>
            <span class="settings-field__body">
              <span class="settings-field__label">Pronouns</span>
              <span class="settings-field__hint">{{ draftPronouns.length }}/{{ messenger.MAX_PROFILE_PRONOUNS_LENGTH }}</span>
            </span>
          </label>
          <div class="settings-inline">
            <input
              v-model="draftPronouns"
              type="text"
              :maxlength="messenger.MAX_PROFILE_PRONOUNS_LENGTH"
              autocomplete="off"
              spellcheck="false"
              placeholder="e.g. they/them"
              class="settings-input"
              @keydown.enter.prevent="saveProfileText"
            />
            <button
              type="button"
              class="btn btn--primary settings-btn"
              :disabled="!profileTextChanged"
              @click="saveProfileText"
            >Save</button>
          </div>
        </div>

        <p class="settings-note">
          Profile image max 2 MB. Banner max 5 MB, PNG/APNG/GIF/JPEG.
        </p>
      </section>

      <section v-else-if="activeSection === 'security'" class="settings-page">
        <div class="settings-group">
          <h4>Account</h4>
          <dl class="settings-kv">
            <div><dt>User ID</dt><dd>{{ messenger.state.userId || "—" }}</dd></div>
            <div><dt>Username</dt><dd>{{ messenger.state.username || "—" }}</dd></div>
          </dl>
          <div class="settings-actions">
            <button type="button" class="btn settings-btn" @click="messenger.downloadRecoveryWords">
              Download recovery words
            </button>
            <button type="button" class="btn settings-btn settings-btn--danger" @click="onLogout">
              Log out
            </button>
          </div>
          <p class="settings-note">
            Recovery words are shown only after account creation or recovery on this browser.
          </p>
        </div>
      </section>

      <section v-else-if="activeSection === 'privacy'" class="settings-page">
        <div class="settings-group">
          <h4>Privacy</h4>
          <label class="settings-check">
            <input
              type="checkbox"
              :checked="messenger.state.deleteMessagesOnLeave"
              @change="messenger.setDeleteMessagesOnLeave(targetChecked($event))"
            />
            <span>Delete local room messages when leaving</span>
          </label>
          <label class="settings-check">
            <input
              type="checkbox"
              :checked="messenger.state.streamerMode"
              @change="messenger.setStreamerMode(targetChecked($event))"
            />
            <span>Streamer mode</span>
          </label>
          <p class="settings-note">
            Streamer mode hides room IDs in the interface while keeping your rooms connected.
          </p>
        </div>
      </section>

      <section v-else-if="activeSection === 'notifications'" class="settings-page">
        <div class="settings-group">
          <h4>Notifications</h4>
          <label class="settings-check">
            <input
              type="checkbox"
              :checked="messenger.state.messageSoundEnabled"
              @change="messenger.setMessageSoundEnabled(targetChecked($event))"
            />
            <span>Play a sound for new messages</span>
          </label>
        </div>
      </section>

      <section v-else-if="activeSection === 'calls'" class="settings-page">
        <div class="settings-group">
          <h4>Calling</h4>
          <label class="settings-check">
            <input type="checkbox" checked disabled />
            <span>Enable incoming calls</span>
          </label>
          <label class="settings-check">
            <input type="checkbox" checked disabled />
            <span>Play calling sounds</span>
          </label>
        </div>

        <div class="settings-group">
          <h4>Devices</h4>
          <label class="settings-select">
            <span>Microphone</span>
            <select :value="messenger.state.selectedAudioInputId" @change="messenger.setAudioInput(targetValue($event))">
              <option value="">System default</option>
              <option v-for="(device, index) in microphones" :key="device.deviceId || `mic-${index}`" :value="device.deviceId">
                {{ deviceLabel(device, `Microphone ${index + 1}`) }}
              </option>
            </select>
          </label>

          <label class="settings-select">
            <span>Speakers</span>
            <select :value="messenger.state.selectedAudioOutputId" @change="messenger.setAudioOutput(targetValue($event))">
              <option value="">System default</option>
              <option v-for="(device, index) in headphones" :key="device.deviceId || `speaker-${index}`" :value="device.deviceId">
                {{ deviceLabel(device, `Output ${index + 1}`) }}
              </option>
            </select>
          </label>

          <p class="settings-note" v-if="messenger.state.audioDevicesPermission !== 'granted'">
            Allow microphone access to reveal the real device names and available inputs/outputs. This does not start a call.
          </p>
          <button
            type="button"
            class="btn settings-btn"
            :disabled="messenger.state.audioDevicesLoading"
            @click="messenger.unlockAudioDevices"
          >
            {{ messenger.state.audioDevicesLoading ? "Checking..." : "Allow and refresh devices" }}
          </button>
        </div>

        <div class="settings-group">
          <h4>Advanced</h4>
          <label class="settings-range">
            <span>Microphone noise threshold</span>
            <small>Raise it to avoid sending background noise.</small>
            <div class="settings-meter">
              <span
                class="settings-meter__bar"
                :style="{ width: `${messenger.state.micTestLevel}%` }"
              ></span>
              <span
                class="settings-meter__threshold"
                :style="{ left: `${messenger.state.microphoneThreshold}%` }"
              ></span>
            </div>
            <input
              type="range"
              min="0"
              max="100"
              step="1"
              :value="messenger.state.microphoneThreshold"
              @input="messenger.setMicrophoneThreshold(targetValue($event))"
            />
            <strong>{{ messenger.state.microphoneThreshold }}</strong>
          </label>
          <button
            type="button"
            class="btn settings-btn"
            :class="{ 'icon-btn--active': messenger.state.micTestActive }"
            :disabled="messenger.state.micTestLoading"
            @click="messenger.startMicTest"
          >
            {{ messenger.state.micTestLoading ? "Starting..." : messenger.state.micTestActive ? "Stop listening" : "Listen and test mic" }}
          </button>
        </div>
      </section>

      <section v-else-if="activeSection === 'admin'" class="settings-page">
        <div class="settings-group">
          <h4>Admin</h4>
          <button type="button" class="btn settings-btn" :disabled="messenger.state.adminLoading" @click="messenger.loadAdminOverview">
            {{ messenger.state.adminLoading ? "Loading..." : "Refresh" }}
          </button>
          <dl class="settings-kv" v-if="messenger.state.adminOverview">
            <div><dt>Online</dt><dd>{{ messenger.state.adminOverview.onlineCount }}</dd></div>
            <div><dt>Users</dt><dd>{{ messenger.state.adminOverview.users?.length || 0 }}</dd></div>
            <div><dt>Rooms</dt><dd>{{ messenger.state.adminOverview.rooms?.length || 0 }}</dd></div>
          </dl>
        </div>

        <div class="settings-group" v-if="messenger.state.adminOverview?.features">
          <h4>Features</h4>
          <label class="settings-check">
            <input
              type="checkbox"
              :checked="messenger.state.adminOverview.features.registerEnabled"
              @change="messenger.setAdminFeature('registerEnabled', targetChecked($event))"
            />
            <span>Registrations</span>
          </label>
          <label class="settings-check">
            <input
              type="checkbox"
              :checked="messenger.state.adminOverview.features.callsEnabled"
              @change="messenger.setAdminFeature('callsEnabled', targetChecked($event))"
            />
            <span>Calls</span>
          </label>
        </div>

        <div class="settings-group" v-if="messenger.state.adminOverview?.users?.length">
          <h4>Users</h4>
          <div class="admin-list">
            <div v-for="user in messenger.state.adminOverview.users" :key="user.id" class="admin-row">
              <div>
                <strong>{{ user.username }}</strong>
                <small>{{ user.id }}</small>
              </div>
              <button
                type="button"
                class="btn settings-btn"
                :class="{ 'settings-btn--danger': !user.disabled }"
                @click="messenger.setAdminUserDisabled(user.id, !user.disabled)"
              >
                {{ user.disabled ? "Enable" : "Disable" }}
              </button>
            </div>
          </div>
        </div>

        <div class="settings-group" v-if="messenger.state.adminOverview?.rooms?.length">
          <h4>Rooms</h4>
          <div class="admin-list">
            <div v-for="room in messenger.state.adminOverview.rooms" :key="room.roomId" class="admin-row">
              <div>
                <strong>{{ messenger.displayRoomName(room.roomId) }}</strong>
                <small>{{ room.messageCount }} messages</small>
              </div>
            </div>
          </div>
          <p class="settings-note">Room previews expose metadata only, never message bodies.</p>
        </div>
      </section>

      <section v-else-if="activeSection === 'backups'" class="settings-page">
        <div class="settings-group">
          <h4>Backups</h4>
          <p class="settings-note">
            Backups include username, room list, message history metadata, reactions, and unread counts.
            Attachment bytes are dropped from persistent storage to keep files small.
          </p>
          <div class="settings-actions">
            <button type="button" class="btn settings-btn" @click="onExport">
              <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M12 3v12"/><path d="m6 9 6-6 6 6"/><path d="M5 21h14"/></svg>
              Export JSON
            </button>
            <button type="button" class="btn settings-btn" @click="onImport">
              <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M12 21V9"/><path d="m6 15 6 6 6-6"/><path d="M5 3h14"/></svg>
              Import JSON
            </button>
            <button type="button" class="btn settings-btn settings-btn--danger" @click="onClear">
              <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M3 6h18"/><path d="M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/><path d="m5 6 1 14a2 2 0 0 0 2 2h8a2 2 0 0 0 2-2l1-14"/></svg>
              Clear all
            </button>
          </div>
        </div>
        <input
          ref="fileInputRef"
          type="file"
          accept="application/json,.json"
          style="display: none"
          @change="onFilePicked"
        />
      </section>

      <section v-else class="settings-page">
        <div class="settings-group">
          <h4>About</h4>
          <dl class="settings-kv">
            <div><dt>Session</dt><dd>{{ messenger.state.uuid || "—" }}</dd></div>
            <div><dt>Status</dt><dd>{{ messenger.connectionLabel.value }}</dd></div>
            <div><dt>Joined rooms</dt><dd>{{ messenger.state.joinedRooms.length }}</dd></div>
            <div><dt>Saved rooms</dt><dd>{{ messenger.state.rooms.length }}</dd></div>
          </dl>
        </div>
      </section>
    </main>
  </div>
</template>
