<script setup lang="ts">
import { computed, onMounted, ref, watch } from "vue";
import { useMessenger } from "@/composables/useMessenger";
import MessengerSidebar from "@/components/MessengerSidebar.vue";
import MemberSidebar from "@/components/MemberSidebar.vue";
import ThreadHeader from "@/components/ThreadHeader.vue";
import MessageList from "@/components/MessageList.vue";
import ComposerBar from "@/components/ComposerBar.vue";
import CallPanel from "@/components/CallPanel.vue";
import SettingsModal from "@/components/SettingsModal.vue";
import OnboardingScreen from "@/components/OnboardingScreen.vue";

const messenger = useMessenger();
const mobileThreadOpen = ref(false);

const needsOnboarding = computed(() => !String(messenger.state.authToken || "").trim() || !String(messenger.state.username || "").trim());
const hasActive = computed(() => !!messenger.roomLabel.value);
const inCall = computed(() => messenger.state.inCall);
const callRoom = computed(() => messenger.state.callRoom);
const callRoomLabel = computed(() => messenger.displayRoomName(callRoom.value));
const callRoomDifferent = computed(() => inCall.value && callRoom.value !== messenger.state.activeRoom);
const callElapsed = computed(() => messenger.formatDuration(messenger.state.callElapsed));

watch(needsOnboarding, (required) => {
  if (!required && !messenger.state.connected && !messenger.state.ws) {
    messenger.connect();
  }
}, { immediate: true });

watch(() => messenger.state.activeRoom, (room) => {
  mobileThreadOpen.value = !!room;
}, { immediate: true });

onMounted(() => {
  if (messenger.state.authToken) messenger.refreshSession();
});

function showConversationList() {
  mobileThreadOpen.value = false;
}

function goToCallRoom() {
  if (callRoom.value && callRoom.value !== messenger.state.activeRoom) {
    messenger.selectConversation(callRoom.value);
  }
}
</script>

<template>
  <OnboardingScreen v-if="needsOnboarding" :messenger="messenger" />

  <div v-else class="app" :class="{ 'is-thread': hasActive && mobileThreadOpen }">
    <MessengerSidebar :messenger="messenger" />

    <Transition name="toast">
      <div v-if="messenger.state.toastMessage" class="toast" role="status" aria-live="polite">
        {{ messenger.state.toastMessage }}
      </div>
    </Transition>

    <main class="thread" v-if="hasActive">
      <div class="thread__shell">
        <section class="thread__main">
          <ThreadHeader :messenger="messenger" @back="showConversationList" />
          <CallPanel :messenger="messenger" />
          <MessageList :messenger="messenger" />
          <ComposerBar :messenger="messenger" />
        </section>

        <MemberSidebar :messenger="messenger" />
      </div>
    </main>

    <div class="no-thread" v-else>
      <div>
        <h2>No conversation selected</h2>
        <p>Pick a room from the list or tap the pencil icon to start one.</p>
      </div>
    </div>

    <div v-if="callRoomDifferent" class="call-pip" @click="goToCallRoom">
      <span class="call-dot"></span>
      <span>In call · {{ callRoomLabel }} · {{ callElapsed }}</span>
      <button type="button" class="btn--ghost" @click.stop="messenger.endCall">end</button>
    </div>

    <SettingsModal :messenger="messenger" />
  </div>
</template>
