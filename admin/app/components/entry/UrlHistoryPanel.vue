<script setup lang="ts">
// 公開URLの変更履歴（05-urls-and-redirects.md、Q39, Q40）。リダイレクトの一覧は M2。
import type { UrlChange } from "~/utils/content";

const props = defineProps<{ workspaceId: string; entryId: string }>();
const { t, locale } = useI18n();
const api = useApi();
const { data: changes, error } = useAsyncData(`url-history:${props.entryId}`, () =>
  api.get<UrlChange[]>(`/workspaces/${props.workspaceId}/entries/${props.entryId}/url-history`),
);
</script>

<template>
  <div class="grid gap-3 text-sm">
    <FormError :error="error" />
    <p v-if="changes && changes.length === 0" class="text-muted-foreground">{{ t("urlHistory.empty") }}</p>
    <ul v-else class="grid gap-2">
      <li v-for="(c, i) in changes ?? []" :key="i" class="grid gap-0.5 rounded-md border p-3">
        <span class="text-muted-foreground">{{ formatDateTime(c.changedAt, locale) }}</span>
        <span class="break-all">{{ c.from }}</span>
        <span class="break-all">→ {{ c.to }}</span>
      </li>
    </ul>
  </div>
</template>
