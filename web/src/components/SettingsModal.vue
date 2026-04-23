<script setup>
import { computed, nextTick, onMounted, onBeforeUnmount, ref, watch } from "vue";

const props = defineProps({
  messenger: { type: Object, required: true }
});

const draftName = ref(props.messenger.state.username || "");
const fileInputRef = ref(null);
const firstInputRef = ref(null);

const isOpen = computed(() => props.messenger.state.settingsOpen);

const nameChanged = computed(() => draftName.value.trim() !== String(props.messenger.state.username || "").trim());
const nameValid = computed(() => draftName.value.trim().length > 0 && draftName.value.trim().length <= 16);

watch(isOpen, async (v) => {
  if (v) {
    draftName.value = props.messenger.state.username || "";
    props.messenger.refreshAudioDevices();
    await nextTick();
    firstInputRef.value?.focus();
    firstInputRef.value?.select();
  }
});

function close() {
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

const microphones = computed(() =>
  props.messenger.state.audioDevices.filter((device) => device.kind === "audioinput")
);
const headphones = computed(() =>
  props.messenger.state.audioDevices.filter((device) => device.kind === "audiooutput")
);

function deviceLabel(device, fallback) {
  return device.label || fallback;
}

function onBackdropClick(event) {
  if (event.target === event.currentTarget) close();
}

function onKey(event) {
  if (!isOpen.value) return;
  if (event.key === "Escape") close();
}

onMounted(() => document.addEventListener("keydown", onKey));
onBeforeUnmount(() => document.removeEventListener("keydown", onKey));
</script>

<template>
  <div v-if="isOpen" class="modal" @mousedown="onBackdropClick">
    <div class="modal__panel" role="dialog" aria-modal="true" aria-labelledby="settings-title">
      <header class="modal__head">
        <h2 id="settings-title">Settings</h2>
        <button class="icon-btn" type="button" aria-label="Close" @click="close">
          <svg viewBox="0 0 24 24"><path d="M18 6 6 18M6 6l12 12"/></svg>
        </button>
      </header>

      <section class="modal__section">
        <h3>Display name</h3>
        <p class="modal__help">
          Other participants see this name. Changing it while connected reconnects the session.
        </p>
        <div class="modal__row">
          <input
            ref="firstInputRef"
            v-model="draftName"
            type="text"
            maxlength="16"
            autocomplete="off"
            spellcheck="false"
            placeholder="e.g. echo"
            class="modal__input"
            @keydown.enter.prevent="saveName"
          />
          <button
            type="button"
            class="btn btn--primary modal__btn"
            :disabled="!nameValid || !nameChanged"
            @click="saveName"
          >Save</button>
        </div>
      </section>

      <section class="modal__section">
        <h3>Local data</h3>
        <p class="modal__help">
          Backups include: username, room list, full message history per room (metadata, text, reactions), and unread counts.
          File-attachment bytes are dropped from the persistent store to keep backups small; messages reference them by metadata.
        </p>
        <div class="modal__buttons">
          <button type="button" class="btn modal__btn" @click="onExport">
            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M12 3v12"/><path d="m6 9 6-6 6 6"/><path d="M5 21h14"/></svg>
            Export JSON
          </button>
          <button type="button" class="btn modal__btn" @click="onImport">
            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M12 21V9"/><path d="m6 15 6 6 6-6"/><path d="M5 3h14"/></svg>
            Import JSON
          </button>
          <button type="button" class="btn modal__btn modal__btn--danger" @click="onClear">
            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M3 6h18"/><path d="M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/><path d="m5 6 1 14a2 2 0 0 0 2 2h8a2 2 0 0 0 2-2l1-14"/></svg>
            Clear all
          </button>
        </div>
        <input
          ref="fileInputRef"
          type="file"
          accept="application/json,.json"
          style="display: none"
          @change="onFilePicked"
        />
      </section>

      <section class="modal__section">
        <h3>Audio</h3>
        <p class="modal__help">
          Choose the microphone and headphone used by calls and voice playback. Increase the threshold to avoid sending room noise.
        </p>
        <div class="modal__field">
          <label for="audio-input">Microphone</label>
          <select id="audio-input" class="modal__input" :value="messenger.state.selectedAudioInputId" @change="messenger.setAudioInput($event.target.value)">
            <option value="">System default</option>
            <option v-for="(device, index) in microphones" :key="device.deviceId || `mic-${index}`" :value="device.deviceId">
              {{ deviceLabel(device, `Microphone ${index + 1}`) }}
            </option>
          </select>
        </div>
        <div class="modal__field">
          <label for="audio-output">Headphones</label>
          <select id="audio-output" class="modal__input" :value="messenger.state.selectedAudioOutputId" @change="messenger.setAudioOutput($event.target.value)">
            <option value="">System default</option>
            <option v-for="(device, index) in headphones" :key="device.deviceId || `speaker-${index}`" :value="device.deviceId">
              {{ deviceLabel(device, `Output ${index + 1}`) }}
            </option>
          </select>
        </div>
        <label class="modal__field">
          <span>Microphone noise threshold: {{ messenger.state.microphoneThreshold }}</span>
          <input
            type="range"
            min="0"
            max="100"
            step="1"
            :value="messenger.state.microphoneThreshold"
            @input="messenger.setMicrophoneThreshold($event.target.value)"
          />
        </label>
        <button type="button" class="btn modal__btn" @click="messenger.refreshAudioDevices">Refresh devices</button>
      </section>

      <section class="modal__section">
        <h3>About</h3>
        <dl class="modal__kv">
          <div><dt>Session</dt><dd>{{ messenger.state.uuid || "—" }}</dd></div>
          <div><dt>Status</dt><dd>{{ messenger.connectionLabel.value }}</dd></div>
          <div><dt>Joined rooms</dt><dd>{{ messenger.state.joinedRooms.length }}</dd></div>
          <div><dt>Saved rooms</dt><dd>{{ messenger.state.rooms.length }}</dd></div>
        </dl>
      </section>
    </div>
  </div>
</template>
