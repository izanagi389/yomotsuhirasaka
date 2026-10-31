<script setup lang="ts">
// 見直しの手がかり（10-llm-and-budget.md「編集者向けのAI支援」、Q69, Q70。M4）。確認期限切れの記事で、編集者がボタンを押したときだけ動く。
// - 古くなっていそうな箇所（バージョン番号、価格、日付、外部リンク）を、下書きにそのまま含まれる語句と理由で並べる
//   （語句が下書きにあることはサーバーが確かめる）。保存済みの下書きが対象。
import { Button } from "@/components/ui/button";
import type { AssistTask } from "~/utils/content";
import AssistStateNote from "~/components/assist/AssistStateNote.vue";

const props = defineProps<{ workspaceId: string; entryId: string }>();
const { t } = useI18n();
const { result, error, start, load } = useAssistTask<AssistTask>(() => `/workspaces/${props.workspaceId}/entries/${props.entryId}/assist/stale`);
onMounted(load);
</script>

<template>
  <section class="grid gap-3 border-t pt-6" data-testid="stale-hints">
    <h3 class="font-medium">{{ t("assist.staleTitle") }}</h3>
    <p class="text-muted-foreground">{{ t("assist.staleHelp") }}</p>
    <div>
      <Button size="sm" variant="outline" :disabled="result?.state === 'running'" data-testid="stale-start" @click="start">{{ t("assist.staleStart") }}</Button>
    </div>
    <p v-if="result?.state === 'disabled'" class="text-muted-foreground">{{ t("assist.disabled") }}</p>
    <AssistStateNote v-if="result" :state="result.state" :period="result.period" />
    <template v-if="result?.state === 'done'">
      <p v-if="(result.hints ?? []).length === 0" class="text-muted-foreground">{{ t("assist.noHints") }}</p>
      <ul v-else class="grid gap-2">
        <li v-for="h in result.hints" :key="h.quote" class="grid gap-1">
          <mark class="w-fit rounded bg-amber-200 px-1 text-foreground dark:bg-amber-500/40">{{ h.quote }}</mark>
          <span class="text-muted-foreground">{{ h.reason }}</span>
        </li>
      </ul>
    </template>
    <FormError :error="error" />
  </section>
</template>
