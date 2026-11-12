<script setup lang="ts">
// 画面の見出し。見出しは明朝、右側に主な操作のボタンを置く。
defineProps<{ title: string; description?: string }>();
</script>

<template>
  <header class="flex flex-wrap items-end justify-between gap-4">
    <div class="grid gap-0.5">
      <h1 class="font-mincho text-2xl font-medium tracking-[0.12em] md:text-[28px]">{{ title }}</h1>
      <p v-if="description" class="text-sm text-muted-foreground">{{ description }}</p>
    </div>
    <div v-if="$slots.actions" class="flex flex-wrap items-center gap-2">
      <slot name="actions" />
    </div>
  </header>
</template>
