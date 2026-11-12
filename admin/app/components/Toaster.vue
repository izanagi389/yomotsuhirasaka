<script setup lang="ts">
import { CircleAlert, CircleCheck, Info, X } from "@lucide/vue";

const { t } = useI18n();
const { toasts, dismiss } = useToast();
</script>

<template>
  <div class="pointer-events-none fixed right-4 bottom-4 z-[60] grid w-[min(360px,calc(100vw-2rem))] gap-2" aria-live="polite" role="status">
    <div
      v-for="toast in toasts"
      :key="toast.id"
      class="pointer-events-auto flex items-start gap-3 rounded-lg border bg-popover p-3 text-sm text-popover-foreground shadow-lg"
      :class="toast.kind === 'error' ? 'border-destructive/40' : ''"
      :data-testid="`toast-${toast.kind}`"
    >
      <CircleCheck v-if="toast.kind === 'success'" class="mt-0.5 size-4 shrink-0 text-emerald-600 dark:text-emerald-400" aria-hidden="true" />
      <CircleAlert v-else-if="toast.kind === 'error'" class="mt-0.5 size-4 shrink-0 text-destructive" aria-hidden="true" />
      <Info v-else class="mt-0.5 size-4 shrink-0 text-primary" aria-hidden="true" />
      <span class="flex-1">{{ toast.message }}</span>
      <button type="button" class="-m-1 rounded p-1 text-muted-foreground hover:text-foreground" :aria-label="t('common.close')" @click="dismiss(toast.id)">
        <X class="size-4" aria-hidden="true" />
      </button>
    </div>
  </div>
</template>
