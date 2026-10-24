<script setup lang="ts">
// 記事の画面の中の「やること」の知らせ（03-content-and-editing.md「やることの見せ方（Q67）」）。記事の画面の上に出す。
// - 「確認期限が切れています」（鮮度のタブへ）
// - 「説明を書いていない訂正があります」（版のタブへ）
// どちらも「訂正と鮮度」の権限を持つ人だけに出す（「やること」の一覧と同じ範囲。INV-TODO-01）。
// データは鮮度・版のタブと同じキーで共有する（タブで記録すると、ここも読み直される）。
import { Button } from "@/components/ui/button";
import type { FreshnessDetail, VersionInfo } from "~/utils/content";

const props = defineProps<{ workspaceId: string; entryId: string }>();
const emit = defineEmits<{ openTab: [tab: string] }>();
const { t } = useI18n();
const api = useApi();
const base = computed(() => `/workspaces/${props.workspaceId}/entries/${props.entryId}`);
const { data: freshness } = useAsyncData(`freshness:${props.entryId}`, () => api.get<FreshnessDetail>(`${base.value}/freshness`));
const { data: versions } = useAsyncData(`versions:${props.entryId}`, () => api.get<VersionInfo[]>(`${base.value}/versions`));
const expired = computed(() => freshness.value?.reviewExpired ?? false);
const unwritten = computed(() => (versions.value ?? []).filter((v) => v.isCorrection && !v.correctionNote).length);
</script>

<template>
  <div v-if="expired || unwritten > 0" class="grid gap-2">
    <div v-if="expired" class="flex flex-wrap items-center gap-3 rounded-md border border-amber-500/50 bg-amber-500/10 p-3 text-sm" role="status" data-testid="notice-expired">
      <span class="flex-1">{{ t("todo.notice.expired") }}</span>
      <Button size="sm" variant="outline" @click="emit('openTab', 'freshness')">{{ t("todo.notice.openFreshness") }}</Button>
    </div>
    <div v-if="unwritten > 0" class="flex flex-wrap items-center gap-3 rounded-md border border-amber-500/50 bg-amber-500/10 p-3 text-sm" role="status" data-testid="notice-unwritten">
      <span class="flex-1">{{ t("todo.notice.unwritten", { n: unwritten }) }}</span>
      <Button size="sm" variant="outline" @click="emit('openTab', 'versions')">{{ t("todo.notice.openVersions") }}</Button>
    </div>
  </div>
</template>
