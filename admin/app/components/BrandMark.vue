<script setup lang="ts">
withDefaults(defineProps<{ size?: number }>(), { size: 28 });
</script>

<template>
  <!-- 管理画面の印（金の線の円と十字）。飾りなので読み上げない -->
  <svg aria-hidden="true" :width="size" :height="size" viewBox="0 0 30 30" fill="none" class="shrink-0 text-kin" stroke="currentColor" stroke-width="1.3">
    <circle cx="15" cy="15" r="13.5" />
    <path d="M15 3c4 4 4 20 0 24M15 3c-4 4-4 20 0 24M3 15h24" />
  </svg>
</template>
