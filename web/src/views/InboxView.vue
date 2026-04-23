<script setup>
import { computed, onMounted } from "vue";
import { useMessenger } from "@/composables/useMessenger";
import MessengerSidebar from "@/components/MessengerSidebar.vue";
import ThreadHeader from "@/components/ThreadHeader.vue";
import MessageList from "@/components/MessageList.vue";
import ComposerBar from "@/components/ComposerBar.vue";
import CallPanel from "@/components/CallPanel.vue";
import SettingsModal from "@/components/SettingsModal.vue";

const messenger = useMessenger();

const hasActive = computed(() => !!messenger.roomLabel.value);
const inCall = computed(() => messenger.state.inCall);
const callRoom = computed(() => messenger.state.callRoom);
const callRoomDifferent = computed(() => inCall.value && callRoom.value !== messenger.state.activeRoom);
const callElapsed = computed(() => messenger.formatDuration(messenger.state.callElapsed));

onMounted(() => {
  if (!messenger.state.connected && !messenger.state.ws) {
    messenger.connect();
  }
});

function goToCallRoom() {
  if (callRoom.value && callRoom.value !== messenger.state.activeRoom) {
    messenger.selectConversation(callRoom.value);
  }
}
</script>

<template>
  <div class="app">
    <MessengerSidebar :messenger="messenger" />

    <Transition name="toast">
      <div v-if="messenger.state.toastMessage" class="toast" role="status" aria-live="polite">
        {{ messenger.state.toastMessage }}
      </div>
    </Transition>

    <main class="thread" v-if="hasActive">
      <ThreadHeader :messenger="messenger" />
      <MessageList :messenger="messenger" />
      <CallPanel :messenger="messenger" />
      <ComposerBar :messenger="messenger" />
    </main>

    <div class="no-thread" v-else>
      <div>
        <h2>No conversation selected</h2>
        <p>Pick a room from the list or tap the pencil icon to start one.</p>
      </div>
    </div>

    <div v-if="callRoomDifferent" class="call-pip" @click="goToCallRoom">
      <span class="call-dot"></span>
      <span>In call · {{ callRoom }} · {{ callElapsed }}</span>
      <button type="button" class="btn--ghost" @click.stop="messenger.endCall">end</button>
    </div>

    <SettingsModal :messenger="messenger" />
  </div>
</template>
