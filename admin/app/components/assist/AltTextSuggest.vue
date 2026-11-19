<script setup lang="ts">
// 代替テキストの案（AI。10-llm-and-budget.md「編集者向けのAI支援」の7、Q75、2026-11-17）。画像1枚ごと。
// - 記事の編集画面（/w/{ws}/entries/{id}）の中だけで出す。案を出せるか（画像を読むモデル・本文にある画像・送れる画像）は
//   サーバーが決め、出せないときはボタンを出さない。
// - ボタンを押すと、画像と前後の段落が外部のLLMの提供会社に送られる（押す前に見積もりを出す）。
// - 「当てる」は代替テキストの入力と同じ操作（親が行う）。当てた代替テキストは、編集者が書いたものと同じに扱う。
// - 案は HTML にせず、文字のまま見せる。
import { Button } from "@/components/ui/button";
import type { AltTextSuggestions } from "~/utils/content";
import AssistStateNote from "~/components/assist/AssistStateNote.vue";

const props = defineProps<{ mediaId: string; canApply: boolean }>();
const emit = defineEmits<{ apply: [text: string] }>();
const { t, locale } = useI18n();
const route = useRoute();
const ws = computed(() => (typeof route.params["ws"] === "string" ? route.params["ws"] : null));
const entry = computed(() => (typeof route.params["id"] === "string" && route.path.includes("/entries/") ? route.params["id"] : null));
const { result, error, start, load } = useAssistTask<AltTextSuggestions>(
  () => `/workspaces/${ws.value}/entries/${entry.value}/assist/alt-text/${encodeURIComponent(props.mediaId)}`,
);
const applied = ref<string | null>(null);
const number = (n: number) => n.toLocaleString(locale.value);

watch(
  () => props.mediaId,
  () => {
    applied.value = null;
    if (ws.value && entry.value && props.mediaId) void load();
  },
  { immediate: true },
);

function apply(text: string) {
  if (!props.canApply) return;
  emit("apply", text);
  applied.value = text;
}
</script>

<template>
  <div v-if="ws && entry && result" class="grid gap-2 text-sm" data-testid="alt-suggest">
    <p v-if="result.reason === 'noVisionModel'" class="text-xs text-muted-foreground">{{ t("assist.altNoVision") }}</p>
    <p v-else-if="result.reason === 'notSendable'" class="text-xs text-muted-foreground">{{ t("assist.altNotSendable") }}</p>
    <div v-else-if="result.available" class="flex flex-wrap items-center gap-2">
      <Button size="sm" variant="outline" :disabled="result.state === 'running'" data-testid="alt-suggest-start" @click="start">{{ t("assist.altSuggest") }}</Button>
      <span class="text-xs text-muted-foreground">
        {{ t("assist.estimate", { input: number(result.estimate.inputTokens), output: number(result.estimate.outputTokens) }) }}
      </span>
    </div>
    <AssistStateNote :state="result.state" :period="result.period" />
    <template v-if="result.state === 'done' && result.suggestions">
      <p v-if="result.suggestions.length === 0" class="text-muted-foreground">{{ t("assist.noCandidates") }}</p>
      <ul class="grid gap-2">
        <li v-for="text in result.suggestions" :key="text" class="grid gap-1 rounded-md border p-2">
          <span>{{ text }}</span>
          <span v-if="applied === text" class="text-xs text-muted-foreground">{{ t("assist.altApplied") }}</span>
          <Button v-else-if="canApply" size="sm" variant="outline" class="w-fit" data-testid="alt-suggest-apply" @click="apply(text)">{{ t("assist.fixApply") }}</Button>
        </li>
      </ul>
      <p class="text-xs text-muted-foreground">{{ t("assist.candidateNote") }}</p>
    </template>
    <FormError :error="error" />
  </div>
</template>
