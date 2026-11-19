<script setup lang="ts">
// 要約とタイトルの案（AI。10-llm-and-budget.md「編集者向けのAI支援」の6、2026-11-17）。編集者がボタンを押したときだけ動く。
// - 保存済みの下書きから、いくつかの案を出す。押す前に、トークン数の上限の見積もりを出す。
// - 「当てる」は、コンテンツタイプの summaryField・titleField のフィールドに、編集者の入力として入れる（自動保存で下書きに入る）。
//   当てる先の指定がないコンテンツタイプでは、この欄を出さない（親が決める）。
// - 案は HTML にせず、文字のまま見せる。
import { Button } from "@/components/ui/button";
import type { DraftingSuggestions } from "~/utils/content";
import AssistStateNote from "~/components/assist/AssistStateNote.vue";

const props = defineProps<{ workspaceId: string; entryId: string; kind: "summary" | "title"; field: string; canApply: boolean; flush?: () => Promise<unknown> }>();
const emit = defineEmits<{ apply: [field: string, text: string] }>();
const { t, locale } = useI18n();
const { result, error, start, load } = useAssistTask<DraftingSuggestions>(() => `/workspaces/${props.workspaceId}/entries/${props.entryId}/assist/${props.kind}`);
onMounted(load);

/** 当てた案（この画面にいる間だけ覚える）。 */
const applied = ref<string | null>(null);
const number = (n: number) => n.toLocaleString(locale.value);

/** 書きかけの下書きを保存してから始める（保存済みの下書きが対象）。 */
async function run() {
  applied.value = null;
  await props.flush?.();
  await start();
}

function apply(text: string) {
  // 当てる先は、サーバーが返したものではなく、画面が知っているコンテンツタイプの指定に限る
  if (!props.canApply || result.value?.field !== props.field) return;
  emit("apply", props.field, text);
  applied.value = text;
}
</script>

<template>
  <section class="grid gap-3" :data-testid="`drafting-${kind}`">
    <h3 class="font-medium">{{ t(kind === "summary" ? "assist.summaryTitle" : "assist.titleTitle") }}</h3>
    <div class="flex flex-wrap items-center gap-3">
      <Button size="sm" variant="outline" :disabled="result?.state === 'running'" :data-testid="`drafting-${kind}-start`" @click="run">
        {{ t(kind === "summary" ? "assist.summaryStart" : "assist.titleStart") }}
      </Button>
      <span v-if="result" class="text-xs text-muted-foreground">
        {{ t("assist.estimate", { input: number(result.estimate.inputTokens), output: number(result.estimate.outputTokens) }) }}
      </span>
    </div>
    <p v-if="result?.state === 'disabled'" class="text-muted-foreground">{{ t("assist.disabled") }}</p>
    <AssistStateNote v-if="result" :state="result.state" :period="result.period" />
    <template v-if="result?.state === 'done' && result.suggestions">
      <p v-if="result.suggestions.length === 0" class="text-muted-foreground">{{ t("assist.noCandidates") }}</p>
      <ul class="grid gap-2">
        <li v-for="text in result.suggestions" :key="text" class="grid gap-1 rounded-md border p-2">
          <span>{{ text }}</span>
          <span v-if="applied === text" class="text-xs text-muted-foreground">{{ t("assist.candidateApplied") }}</span>
          <Button v-else-if="canApply" size="sm" variant="outline" class="w-fit" :data-testid="`drafting-${kind}-apply`" @click="apply(text)">{{ t("assist.fixApply") }}</Button>
        </li>
      </ul>
      <p class="text-xs text-muted-foreground">{{ t("assist.candidateNote") }}</p>
    </template>
    <FormError :error="error" />
  </section>
</template>
