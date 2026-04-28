<script setup lang="ts">
import { computed, nextTick, onMounted, ref } from "vue";

const props = defineProps({
  messenger: { type: Object, required: true }
});

const mode = ref("login");
const username = ref(String(props.messenger.state.username || ""));
const password = ref("");
const newPassword = ref("");
const recoveryWords = ref("");
const inputRef = ref<HTMLInputElement | null>(null);

const cleanUsername = computed(() => username.value.trim().toLowerCase());
const usernameError = computed(() => props.messenger.validateUsername(cleanUsername.value));
const passwordValid = computed(() => password.value.length >= 8 && password.value.length <= 128);
const newPasswordValid = computed(() => newPassword.value.length >= 8 && newPassword.value.length <= 128);
const canSubmit = computed(() => {
  if (usernameError.value) return false;
  if (mode.value === "recover") return recoveryWords.value.trim().length > 0 && newPasswordValid.value;
  return passwordValid.value;
});
const previewAccent = computed(() => props.messenger.accentFor(cleanUsername.value || "you"));
const errorMessage = computed(() => String(props.messenger.state.lastError || "").trim());

function initialsOf(name: string) {
  const trimmed = String(name || "?").trim();
  return (trimmed.slice(0, 2) || "?").toUpperCase();
}

function setMode(next: string) {
  mode.value = next;
  props.messenger.state.lastError = "";
  nextTick(() => inputRef.value?.focus());
}

async function submit() {
  props.messenger.state.lastError = "";
  if (mode.value === "register") {
    await props.messenger.registerAccount(cleanUsername.value, password.value);
  } else if (mode.value === "recover") {
    await props.messenger.recoverAccount(cleanUsername.value, recoveryWords.value, newPassword.value);
  } else {
    await props.messenger.loginAccount(cleanUsername.value, password.value);
  }
}

onMounted(() => nextTick(() => inputRef.value?.focus()));
</script>

<template>
  <section class="onboarding">
    <div class="onboarding__card">
      <div class="onboarding__hero">
        <p class="onboarding__eyebrow">Welcome to QxProtocol</p>
        <h1>{{ mode === "register" ? "Create account" : mode === "recover" ? "Recover account" : "Log in" }}</h1>
        <p class="onboarding__copy">
          Accounts are required to access rooms, presence and calls.
        </p>
      </div>

      <div class="onboarding__body">
        <div class="onboarding__preview">
          <span class="avatar avatar--lg" :class="`avatar--${previewAccent}`">{{ initialsOf(cleanUsername || "You") }}</span>
          <div>
            <strong>{{ cleanUsername || "your.username" }}</strong>
            <small>2-32 chars, lowercase, numbers, _ and .</small>
          </div>
        </div>

        <div class="onboarding__tabs" role="tablist" aria-label="Authentication mode">
          <button type="button" :class="{ 'is-active': mode === 'login' }" @click="setMode('login')">Login</button>
          <button type="button" :class="{ 'is-active': mode === 'register' }" @click="setMode('register')">Register</button>
          <button type="button" :class="{ 'is-active': mode === 'recover' }" @click="setMode('recover')">Recover</button>
        </div>

        <form class="onboarding__form" @submit.prevent="submit">
          <label class="onboarding__field" for="onboarding-username">
            <span>Username</span>
            <input
              id="onboarding-username"
              ref="inputRef"
              v-model="username"
              type="text"
              maxlength="32"
              autocomplete="username"
              spellcheck="false"
              placeholder="alex.qxp"
            />
          </label>

          <label v-if="mode !== 'recover'" class="onboarding__field" for="onboarding-password">
            <span>Password</span>
            <input
              id="onboarding-password"
              v-model="password"
              type="password"
              maxlength="128"
              autocomplete="current-password"
              placeholder="8 characters minimum"
            />
          </label>

          <label v-if="mode === 'recover'" class="onboarding__field" for="onboarding-recovery">
            <span>Recovery words</span>
            <textarea
              id="onboarding-recovery"
              v-model="recoveryWords"
              rows="4"
              autocomplete="off"
              spellcheck="false"
              placeholder="Paste your 16 recovery words"
            ></textarea>
          </label>

          <label v-if="mode === 'recover'" class="onboarding__field" for="onboarding-new-password">
            <span>New password</span>
            <input
              id="onboarding-new-password"
              v-model="newPassword"
              type="password"
              maxlength="128"
              autocomplete="new-password"
              placeholder="8 characters minimum"
            />
          </label>

          <div class="onboarding__meta">
            <span>{{ usernameError || (mode === "register" ? "Recovery words download after signup" : "Session required") }}</span>
            <span>{{ cleanUsername.length }}/32</span>
          </div>

          <p v-if="errorMessage" class="onboarding__error">{{ errorMessage }}</p>

          <button class="btn btn--primary onboarding__submit" type="submit" :disabled="!canSubmit || messenger.state.authLoading">
            {{ messenger.state.authLoading ? "Please wait..." : mode === "register" ? "Create account" : mode === "recover" ? "Reset password" : "Log in" }}
          </button>
        </form>
      </div>
    </div>
  </section>
</template>
