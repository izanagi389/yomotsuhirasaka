<script setup lang="ts">
// 何もないときの表示。七宝を薄く敷く（和柄はログイン画面と、この表示だけ）。
defineProps<{ title: string; description?: string }>();
</script>

<template>
  <div class="relative grid justify-items-center gap-1.5 overflow-hidden rounded-xl border border-dashed px-6 py-10 text-center text-sm">
    <WaPattern kind="shippo" class="text-primary opacity-[0.07]" />
    <span class="relative font-medium">{{ title }}</span>
    <span v-if="description" class="relative text-muted-foreground">{{ description }}</span>
    <div v-if="$slots.default" class="relative mt-2"><slot /></div>
  </div>
</template>
