<script setup lang="ts">
// 書くべき記事の構成案（10-llm-and-budget.md「編集者向けのAI支援」、Q69, Q70。M4）。質問ログの候補1つから、編集者がボタンを
// 押したときだけ作る。構成案は Markdown の文で、HTML にせず文字のまま見せる（v-html は使わない）。記事は作らない。
// 質問文を使うので、作るのも見るのも重い権限 viewQuestionLog と再認証が要る（サーバー。再認証のダイアログは useApi が出す）。
import { Button } from "@/components/ui/button";
import type { AssistTask } from "~/utils/content";
import AssistStateNote from "~/components/assist/AssistStateNote.vue";

const props = defineProps<{ workspaceId: string; candidateId: string }>();
const { t } = useI18n();
const { result, error, start } = useAssistTask<AssistTask>(
  () => `/workspaces/${props.workspaceId}/question-log/candidates/${props.candidateId}/outline`,
);
const copied = ref(false);
async function copy() {
  if (!result.value?.outline) return;
  await navigator.clipboard.writeText(result.value.outline);
  copied.value = true;
}
</script>

<template>
  <div class="grid w-full gap-2" data-testid="outline">
    <div>
      <Button variant="outline" size="sm" :disabled="result?.state === 'running'" data-testid="outline-start" @click="start">{{ t("assist.outlineStart") }}</Button>
    </div>
    <p v-if="result?.state === 'disabled'" class="text-muted-foreground">{{ t("assist.disabled") }}</p>
    <AssistStateNote v-if="result" :state="result.state" :period="result.period" />
    <template v-if="result?.state === 'done' && result.outline">
      <pre class="whitespace-pre-wrap break-words rounded-md bg-muted p-3 font-sans">{{ result.outline }}</pre>
      <div class="flex items-center gap-2">
        <Button variant="outline" size="sm" @click="copy">{{ t("assist.copy") }}</Button>
        <span v-if="copied" class="text-muted-foreground" role="status">{{ t("assist.copied") }}</span>
      </div>
      <p class="text-xs text-muted-foreground">{{ t("assist.outlineNote") }}</p>
    </template>
    <FormError :error="error" />
  </div>
</template>
