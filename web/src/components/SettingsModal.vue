<script setup lang="ts">
import { computed, nextTick, onMounted, onBeforeUnmount, ref, watch } from "vue";

const props = defineProps({
  messenger: { type: Object, required: true }
});

const draftName = ref(props.messenger.state.username || "");
const fileInputRef = ref(null);
const firstInputRef = ref(null);
const activeSection = ref("profile");

const isOpen = computed(() => props.messenger.state.settingsOpen);

const nameChanged = computed(() => draftName.value.trim() !== String(props.messenger.state.username || "").trim());
const nameValid = computed(() => draftName.value.trim().length > 0 && draftName.value.trim().length <= 16);
const meAccent = computed(() => props.messenger.accentFor(props.messenger.state.username || "you"));
const meInitials = computed(() => initialsOf(props.messenger.state.username));

const sections = [
  { id: "profile", label: "Profile" },
  { id: "privacy", label: "Privacy" },
  { id: "notifications", label: "Notifications" },
  { id: "calls", label: "Calls" },
  { id: "backups", label: "Backups" },
  { id: "about", label: "About" }
];

watch(isOpen, async (v) => {
  if (v) {
    draftName.value = props.messenger.state.username || "";
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
  else props.messenger.stopMicTest();
  if (section === "profile") {
    await nextTick();
    firstInputRef.value?.focus();
  }
});

function close() {
  props.messenger.stopMicTest();
  props.messenger.state.settingsOpen = false;
}

function saveName() {
  if (!nameValid.value || !nameChanged.value) return;
  props.messenger.changeUsername(draftName.value.trim());
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
          <span class="avatar settings-profile__avatar" :class="`avatar--${meAccent}`">{{ meInitials }}</span>
          <button type="button" class="btn settings-profile__photo">Edit photo</button>
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
              maxlength="16"
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

        <p class="settings-note">
          Your profile and changes to it will be visible to people you message.
        </p>
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
