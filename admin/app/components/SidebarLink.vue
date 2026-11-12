<script setup lang="ts">
// サイドバーの1項目。今いる画面には、朱の栞の印を付ける。
import type { Component } from "vue";

defineProps<{ to: string; label: string; icon?: Component; count?: number; active?: boolean }>();
</script>

<template>
  <NuxtLink
    :to="to"
    class="relative flex min-h-9 items-center gap-2.5 rounded-lg px-2.5 py-2 outline-none hover:bg-sidebar-active hover:text-sidebar-strong focus-visible:ring-2 focus-visible:ring-ring"
    :class="active ? 'bg-sidebar-active font-medium text-sidebar-strong' : ''"
    :aria-current="active ? 'page' : undefined"
  >
    <svg v-if="active" aria-hidden="true" width="6" height="18" viewBox="0 0 8 22" class="absolute top-1/2 -left-1.5 -translate-y-1/2 text-shu">
      <path d="M0 0H8V22L4 18L0 22Z" fill="currentColor" />
    </svg>
    <component :is="icon" v-if="icon" class="size-4.5 shrink-0" aria-hidden="true" />
    <span class="truncate">{{ label }}</span>
    <span v-if="count" class="ml-auto rounded-full bg-shu px-1.5 text-[11px] font-bold text-white" data-testid="todo-count">{{ count }}</span>
  </NuxtLink>
</template>
