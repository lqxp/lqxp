<script setup lang="ts">
defineProps({
  src: { type: String, required: true },
  filename: { type: String, default: "Video" },
  sizeLabel: { type: String, default: "" }
});
</script>

<template>
  <figure class="video-player">
    <video class="video-player__media" :src="src" controls preload="metadata" playsinline></video>
    <figcaption class="video-player__meta">
      <span class="video-player__title">{{ filename }}</span>
      <span v-if="sizeLabel" class="video-player__size">{{ sizeLabel }}</span>
    </figcaption>
  </figure>
</template>
