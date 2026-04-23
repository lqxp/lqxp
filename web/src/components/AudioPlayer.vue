<script setup>
import { computed, onBeforeUnmount, ref, watch } from "vue";

const props = defineProps({
  src: { type: String, required: true },
  filename: { type: String, default: "Audio" },
  sizeLabel: { type: String, default: "" },
  fallbackDuration: { type: String, default: "" },
  messenger: { type: Object, default: null }
});

const audioRef = ref(null);
const isPlaying = ref(false);
const currentTime = ref(0);
const duration = ref(0);
const canPlay = ref(false);
let frameId = 0;

const progress = computed(() => {
  if (!duration.value) return 0;
  return Math.min(100, (currentTime.value / duration.value) * 100);
});

const elapsedLabel = computed(() => formatClock(currentTime.value));
const durationLabel = computed(() => {
  if (duration.value) return formatClock(duration.value);
  return props.fallbackDuration || "--:--";
});

function formatClock(seconds) {
  const value = Math.max(0, Math.floor(Number(seconds) || 0));
  const minutes = Math.floor(value / 60);
  const rest = String(value % 60).padStart(2, "0");
  return `${minutes}:${rest}`;
}

function syncAudioState() {
  const audio = audioRef.value;
  if (!audio) return;
  currentTime.value = audio.currentTime || 0;
  duration.value = Number.isFinite(audio.duration) ? audio.duration : 0;
}

function stopProgressLoop() {
  if (!frameId) return;
  cancelAnimationFrame(frameId);
  frameId = 0;
}

function startProgressLoop() {
  stopProgressLoop();

  const tick = () => {
    syncAudioState();
    const audio = audioRef.value;
    if (!audio || audio.paused || audio.ended) {
      frameId = 0;
      return;
    }
    frameId = requestAnimationFrame(tick);
  };

  frameId = requestAnimationFrame(tick);
}

async function togglePlayback() {
  const audio = audioRef.value;
  if (!audio) return;

  if (audio.paused) {
    try {
      await audio.play();
    } catch {
      isPlaying.value = false;
    }
  } else {
    audio.pause();
  }
}

function seek(event) {
  const audio = audioRef.value;
  if (!audio || !duration.value) return;
  const nextTime = (Number(event.target.value) / 100) * duration.value;
  audio.currentTime = nextTime;
  currentTime.value = nextTime;
}

function onLoadedMetadata() {
  props.messenger?.applyAudioOutput?.(audioRef.value);
  canPlay.value = true;
  syncAudioState();
}

watch(
  () => props.messenger?.state.selectedAudioOutputId,
  () => props.messenger?.applyAudioOutput?.(audioRef.value)
);

function onPlay() {
  isPlaying.value = true;
  startProgressLoop();
}

function onPause() {
  isPlaying.value = false;
  stopProgressLoop();
  syncAudioState();
}

function onEnded() {
  isPlaying.value = false;
  stopProgressLoop();
  syncAudioState();
}

watch(
  () => props.src,
  () => {
    stopProgressLoop();
    isPlaying.value = false;
    currentTime.value = 0;
    duration.value = 0;
    canPlay.value = false;
  }
);

onBeforeUnmount(() => {
  stopProgressLoop();
  const audio = audioRef.value;
  if (audio) audio.pause();
});
</script>

<template>
  <div class="audio-player">
    <audio
      ref="audioRef"
      :src="src"
      preload="metadata"
      @loadedmetadata="onLoadedMetadata"
      @durationchange="syncAudioState"
      @timeupdate="syncAudioState"
      @play="onPlay"
      @pause="onPause"
      @ended="onEnded"
    ></audio>

    <button class="audio-player__play" type="button" :aria-label="isPlaying ? 'Pause audio' : 'Play audio'" @click="togglePlayback">
      <svg v-if="isPlaying" viewBox="0 0 24 24" aria-hidden="true">
        <path d="M8 5h3v14H8zM13 5h3v14h-3z"/>
      </svg>
      <svg v-else viewBox="0 0 24 24" aria-hidden="true">
        <path d="M8 5v14l11-7z"/>
      </svg>
    </button>

    <div class="audio-player__body">
      <div class="audio-player__top">
        <span class="audio-player__title">{{ filename }}</span>
        <span class="audio-player__duration">{{ elapsedLabel }} / {{ durationLabel }}</span>
      </div>

      <label class="audio-player__seek">
        <span class="sr-only">Audio progress</span>
        <input
          type="range"
          min="0"
          max="100"
          step="0.1"
          :value="progress"
          :disabled="!canPlay"
          :style="{ '--progress': `${progress}%` }"
          @input="seek"
        />
      </label>

      <div v-if="sizeLabel" class="audio-player__meta">{{ sizeLabel }}</div>
    </div>
  </div>
</template>
