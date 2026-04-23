<script setup>
import { computed } from "vue";
import AudioPlayer from "@/components/AudioPlayer.vue";

const props = defineProps({
  message: { type: Object, required: true },
  messenger: { type: Object, required: true },
  position: { type: String, default: "single" },
  showAuthor: { type: Boolean, default: true },
  showAvatar: { type: Boolean, default: true }
});

const isOwn = computed(() => props.messenger.isOwnMessage(props.message));

const runClass = computed(() => {
  switch (props.position) {
    case "start": return "is-run-start";
    case "mid": return "is-run-mid";
    case "end": return "is-run-end";
    default: return "is-single";
  }
});

const avatarInitials = computed(() => {
  const n = String(props.message.username || "?").trim();
  const parts = n.split(/[\s\-_]+/).slice(0, 2);
  if (parts.length === 2 && parts[1]) return (parts[0][0] + parts[1][0]).toUpperCase();
  return n.slice(0, 2).toUpperCase();
});

const avatarAccent = computed(() => props.messenger.accentFor(props.message.username || ""));

const showTimestamp = computed(() => props.position === "end" || props.position === "single");

const attachmentUrl = computed(() => props.messenger.attachmentUrlFor(props.message));
const attachmentKind = computed(() => props.message.kind);
const jumbo = computed(() => props.message.jumboEmoji && !props.message.deleted);
const deleted = computed(() => props.message.deleted);
const preview = computed(() => props.message.preview);

function download() {
  if (!attachmentUrl.value || !props.message.attachment) return;
  const a = document.createElement("a");
  a.href = attachmentUrl.value;
  a.download = props.message.attachment.filename || "file";
  document.body.appendChild(a);
  a.click();
  a.remove();
}

function onDelete() {
  if (!isOwn.value || deleted.value) return;
  if (!confirm("Delete this message for everyone in the room?")) return;
  props.messenger.deleteMessage(props.message);
}
</script>

<template>
  <article
    class="msg"
    :class="[
      { 'is-own': isOwn, 'is-jumbo': jumbo, 'is-deleted': deleted },
      runClass
    ]"
  >
    <span
      v-if="!isOwn && showAvatar"
      class="msg__avatar"
      :class="`avatar--${avatarAccent}`"
    >{{ avatarInitials }}</span>
    <span v-else-if="!isOwn" class="msg__spacer"></span>

    <div v-if="jumbo" class="jumbo">
      <div v-if="showAuthor && !isOwn" class="jumbo__author">{{ message.username }}</div>
      <div class="jumbo__glyph">{{ message.text }}</div>
      <span v-if="showTimestamp" class="jumbo__time">{{ messenger.formatTime(message.timestamp) }}</span>
      <div v-if="message.reactions.length" class="reactions reactions--standalone">
        <button
          v-for="reaction in message.reactions"
          :key="`${message.messageId}-${reaction.emoji}`"
          class="reaction"
          type="button"
          @click="messenger.toggleReaction(message, reaction.emoji)"
        >
          <span>{{ reaction.emoji }}</span>
          <span v-if="reaction.count > 1">{{ reaction.count }}</span>
        </button>
      </div>
      <div class="bubble-actions">
        <div class="pick">
          <button
            v-for="emoji in messenger.QUICK_REACTIONS"
            :key="`pick-${emoji}`"
            type="button"
            @click="messenger.toggleReaction(message, emoji)"
          >{{ emoji }}</button>
          <button
            v-if="isOwn"
            type="button"
            class="pick__delete"
            aria-label="Delete"
            @click="onDelete"
          >
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><polyline points="3 6 5 6 21 6"/><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/></svg>
          </button>
        </div>
      </div>
    </div>

    <div
      v-else
      class="bubble"
      :class="{
        'bubble--media': attachmentKind === 'image',
        'bubble--deleted': deleted
      }"
    >
      <div class="bubble-actions">
        <div class="pick" role="group" aria-label="React">
          <button
            v-for="emoji in messenger.QUICK_REACTIONS"
            :key="`pick-${emoji}`"
            type="button"
            @click="messenger.toggleReaction(message, emoji)"
          >{{ emoji }}</button>
          <button
            v-if="isOwn && !deleted"
            type="button"
            class="pick__delete"
            aria-label="Delete"
            @click="onDelete"
          >
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><polyline points="3 6 5 6 21 6"/><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/></svg>
          </button>
        </div>
      </div>

      <div v-if="showAuthor && !isOwn" class="bubble__author">{{ message.username }}</div>

      <template v-if="deleted">
        <div class="bubble__text bubble__text--deleted">Message deleted</div>
      </template>

      <template v-else-if="attachmentKind === 'image' && attachmentUrl">
        <a :href="attachmentUrl" :download="message.attachment.filename" class="att-image-link">
          <img :src="attachmentUrl" :alt="message.attachment.filename" class="att-image" />
        </a>
        <div v-if="message.text" class="bubble__text">{{ message.text }}</div>
      </template>

      <template v-else-if="attachmentKind === 'audio' && attachmentUrl">
        <AudioPlayer
          :src="attachmentUrl"
          :filename="message.attachment.filename"
          :size-label="messenger.formatSize(message.attachment.size)"
          :fallback-duration="message.voiceDuration || ''"
        />
        <div v-if="message.text && !message.text.startsWith('[voice:')" class="bubble__text">{{ message.text }}</div>
      </template>

      <template v-else-if="attachmentKind === 'file' && message.attachment">
        <button class="att-file" type="button" @click="download" :disabled="!attachmentUrl">
          <span class="att-file-icon">
            <svg viewBox="0 0 24 24" width="20" height="20" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8Z"/><path d="M14 2v6h6"/></svg>
          </span>
          <span class="att-file-meta">
            <span class="att-file-name">{{ message.attachment.filename }}</span>
            <span class="att-file-sub">
              {{ messenger.formatSize(message.attachment.size) }}
              <span v-if="!attachmentUrl"> · expired</span>
            </span>
          </span>
          <span v-if="attachmentUrl" class="att-file-dl">
            <svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><polyline points="7 10 12 15 17 10"/><line x1="12" y1="15" x2="12" y2="3"/></svg>
          </span>
        </button>
        <div v-if="message.text" class="bubble__text">{{ message.text }}</div>
      </template>

      <template v-else>
        <div class="bubble__text">{{ message.text }}</div>
      </template>

      <a v-if="preview && !deleted" :href="preview.url" target="_blank" rel="noopener noreferrer" class="embed">
        <div v-if="preview.image" class="embed__media">
          <img :src="preview.image" :alt="preview.title || preview.url" loading="lazy" referrerpolicy="no-referrer" />
        </div>
        <div class="embed__body">
          <div v-if="preview.siteName" class="embed__site">{{ preview.siteName }}</div>
          <div v-if="preview.title" class="embed__title">{{ preview.title }}</div>
          <div v-if="preview.description" class="embed__desc">{{ preview.description }}</div>
        </div>
      </a>

      <span v-if="showTimestamp && !deleted" class="bubble__time">{{ messenger.formatTime(message.timestamp) }}</span>

      <div v-if="message.reactions.length && !deleted" class="reactions">
        <button
          v-for="reaction in message.reactions"
          :key="`${message.messageId}-${reaction.emoji}`"
          class="reaction"
          type="button"
          @click="messenger.toggleReaction(message, reaction.emoji)"
        >
          <span>{{ reaction.emoji }}</span>
          <span v-if="reaction.count > 1">{{ reaction.count }}</span>
        </button>
      </div>
    </div>
  </article>
</template>
