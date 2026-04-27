<script setup lang="ts">
import { computed, nextTick, onMounted, ref } from "vue";

const props = defineProps({
  messenger: { type: Object, required: true }
});

const username = ref(String(props.messenger.state.username || ""));
const inputRef = ref<HTMLInputElement | null>(null);

const trimmedUsername = computed(() => username.value.trim());
const canSubmit = computed(() => trimmedUsername.value.length > 0 && trimmedUsername.value.length <= 16);
const previewName = computed(() => trimmedUsername.value || "You");
const previewAccent = computed(() => props.messenger.accentFor(previewName.value));
const errorMessage = computed(() => String(props.messenger.state.lastError || "").trim());

function initialsOf(name: string) {
  const trimmed = String(name || "?").trim();
  if (!trimmed) return "?";
  const parts = trimmed.split(/[\s\-_]+/).slice(0, 2);
  if (parts.length === 2 && parts[1]) return (parts[0][0] + parts[1][0]).toUpperCase();
  return trimmed.slice(0, 2).toUpperCase();
}

function clearError() {
  if (props.messenger.state.lastError) {
    props.messenger.state.lastError = "";
  }
}

async function focusInput() {
  await nextTick();
  inputRef.value?.focus();
  inputRef.value?.select();
}

async function submit() {
  clearError();
  if (!props.messenger.changeUsername(trimmedUsername.value)) {
    await focusInput();
    return;
  }
  props.messenger.connect();
}

onMounted(focusInput);
</script>

<template>
  <section class="onboarding">
    <div class="onboarding__card">
      <div class="onboarding__hero">
        <p class="onboarding__eyebrow">Welcome to QxProtocol</p>
        <h1>Choose your username</h1>
        <p class="onboarding__copy">
          Pick the name people will see in chats and calls. You can change it later from settings.
        </p>
      </div>

      <div class="onboarding__body">
        <div class="onboarding__preview">
          <span class="avatar avatar--lg" :class="`avatar--${previewAccent}`">{{ initialsOf(previewName) }}</span>
          <div>
            <strong>{{ previewName }}</strong>
            <small>Visible to everyone in your rooms</small>
          </div>
        </div>

        <form class="onboarding__form" @submit.prevent="submit">
          <label class="onboarding__field" for="onboarding-username">
            <span>Username</span>
            <input
              id="onboarding-username"
              ref="inputRef"
              v-model="username"
              type="text"
              maxlength="16"
              autocomplete="username"
              spellcheck="false"
              placeholder="e.g. alex"
              @input="clearError"
            />
          </label>

          <div class="onboarding__meta">
            <span>1 to 16 characters</span>
            <span>{{ trimmedUsername.length }}/16</span>
          </div>

          <p v-if="errorMessage" class="onboarding__error">{{ errorMessage }}</p>

          <button class="btn btn--primary onboarding__submit" type="submit" :disabled="!canSubmit">
            Continue
          </button>
        </form>
      </div>
    </div>
  </section>
</template>
