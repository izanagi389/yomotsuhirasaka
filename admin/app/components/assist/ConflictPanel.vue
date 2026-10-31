<script setup lang="ts">
// 公開の確認画面の「矛盾の検知」（10-llm-and-budget.md「編集者向けのAI支援」、Q69, Q70。M4）。
// - 確認画面を開いたら自動で始める（同じ下書きの結果があれば、サーバーはそれを返す）。支援が無効なら何も出さない。
// - 意味の近い他の記事との食い違いを並べ、相手の記事を開けるようにする。公開は止めない（結果を待たない。INV-BUD-02）。
import type { ConflictCheck } from "~/utils/content";
import AssistStateNote from "~/components/assist/AssistStateNote.vue";

const props = defineProps<{ workspaceId: string; entryId: string }>();
const { t } = useI18n();
const { result, error, start } = useAssistTask<ConflictCheck>(() => `/workspaces/${props.workspaceId}/entries/${props.entryId}/assist/conflicts`);
onMounted(start);
</script>

<template>
  <section v-if="result && result.state !== 'disabled'" class="grid gap-2" data-testid="publish-conflicts">
    <h3 class="font-medium">{{ t("assist.conflictsTitle") }}</h3>
    <AssistStateNote :state="result.state" :period="result.period" />
    <template v-if="result.state === 'done'">
      <p v-if="result.conflicts.length === 0" class="text-muted-foreground">{{ t("assist.noConflicts") }}</p>
      <ul v-else class="grid gap-2">
        <li v-for="c in result.conflicts" :key="`${c.entryId}:${c.explanation}`" class="rounded-md border border-amber-500/50 bg-amber-500/10 p-3">
          <p>{{ c.explanation }}</p>
          <NuxtLink :to="`/w/${workspaceId}/entries/${c.entryId}`" target="_blank" class="text-primary underline">{{ t("assist.openOther") }}</NuxtLink>
        </li>
      </ul>
      <p class="text-xs text-muted-foreground">{{ t("assist.conflictsNote") }}</p>
    </template>
  </section>
  <FormError :error="error" />
</template>
