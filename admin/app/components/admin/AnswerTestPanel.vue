<script setup lang="ts">
// 回答のテスト（09-qa-chatbot.md「根拠と判定」「語り口」。M4）。チャットボットのページで、保存する前の語り口でも試せる。
// - 検索された断片と点数、LLM の判定の宣言、LLM の出力そのもの（検査の前）、最終の結果を並べる
// - 拒否する語・マスク・予算・回答の検査は本番と同じ。回答キャッシュは使わず、質問ログにも残らない（サーバー）
// - 文字はすべて文字として出す（回答の Markdown も HTML にしない。v-html は使わない）
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import type { AnswerSettings, AnswerTestResult } from "~/utils/content";

const MAX_QUESTION = 500;

const props = defineProps<{
  workspaceId: string;
  /** テンプレートのロケール（既定のロケールが先）。 */
  locales: string[];
  /** いま画面で編集している語り口（保存していなくてよい）。 */
  tone: Pick<AnswerSettings, "politeness" | "length" | "freePrompt">;
}>();
const { t, te } = useI18n();
const api = useApi();
const question = ref("");
const locale = ref(props.locales[0] ?? "");
watch(
  () => props.locales,
  (l) => {
    if (!l.includes(locale.value)) locale.value = l[0] ?? "";
  },
);
const running = ref(false);
const error = ref<unknown>(null);
const result = ref<AnswerTestResult | null>(null);
const chars = (s: string) => [...s].length;

async function run() {
  if (!question.value.trim() || chars(question.value) > MAX_QUESTION) return;
  running.value = true;
  error.value = null;
  result.value = null;
  try {
    const { politeness, length, freePrompt } = props.tone;
    result.value = await api.post<AnswerTestResult>(`/workspaces/${props.workspaceId}/answer-test`, {
      question: question.value,
      locale: locale.value,
      tone: { politeness, length, freePrompt: { enabled: freePrompt.enabled, text: freePrompt.text } },
    });
  } catch (e) {
    error.value = e;
  } finally {
    running.value = false;
  }
}
const score = (v: number | null) => (v === null ? "—" : v.toFixed(3));
const errorText = (code: string) => (te(`answerTest.errors.${code}`) ? t(`answerTest.errors.${code}`) : code);
const declared = computed(() => new Set(result.value?.declaration?.sources ?? []));
</script>

<template>
  <section class="grid gap-4 text-sm" data-testid="answer-test">
    <div class="grid gap-1">
      <h3 class="font-medium">{{ t("answerTest.title") }}</h3>
      <p class="text-muted-foreground">{{ t("answerTest.help") }}</p>
    </div>
    <form class="grid gap-3" @submit.prevent="run">
      <div class="grid gap-1.5">
        <Label for="answer-test-question">{{ t("answerTest.question", { max: MAX_QUESTION }) }}</Label>
        <Textarea id="answer-test-question" v-model="question" rows="2" :disabled="running" :aria-invalid="chars(question) > MAX_QUESTION" />
      </div>
      <div class="flex flex-wrap items-center gap-3">
        <Label for="answer-test-locale">{{ t("answerTest.locale") }}</Label>
        <select id="answer-test-locale" v-model="locale" class="h-9 rounded-md border bg-transparent px-3" :disabled="running">
          <option v-for="l in locales" :key="l" :value="l">{{ l }}</option>
        </select>
        <Button type="submit" :disabled="running || !question.trim() || chars(question) > MAX_QUESTION" data-testid="answer-test-run">
          {{ running ? t("answerTest.running") : t("answerTest.run") }}
        </Button>
      </div>
      <p class="text-muted-foreground">{{ t("answerTest.note") }}</p>
    </form>
    <FormError :error="error" />

    <div v-if="result" class="grid gap-5" data-testid="answer-test-result">
      <div class="grid gap-1">
        <h4 class="font-medium">{{ t("answerTest.masked") }}</h4>
        <p class="whitespace-pre-wrap break-words rounded-md bg-muted p-2">{{ result.maskedQuestion }}</p>
      </div>

      <p v-if="result.error" class="text-destructive" role="alert" data-testid="answer-test-error">{{ errorText(result.error.code) }}</p>

      <div v-if="!result.error || result.fragments.length > 0" class="grid gap-2">
        <h4 class="font-medium">{{ t("answerTest.fragments") }}</h4>
        <p v-if="result.fragments.length === 0" class="text-muted-foreground">{{ t("answerTest.noFragments") }}</p>
        <ol v-else class="grid gap-2">
          <li v-for="f in result.fragments" :key="f.n" class="grid gap-1 rounded-md border p-3">
            <div class="flex flex-wrap items-center gap-2">
              <Badge variant="outline">[{{ f.n }}]</Badge>
              <Badge v-if="declared.has(f.n)">{{ t("answerTest.declared") }}</Badge>
              <span class="font-medium">{{ f.breadcrumb.join(" › ") || f.entryId }}</span>
              <span class="text-muted-foreground">{{ f.locale }}<template v-if="f.anchor"> #{{ f.anchor }}</template></span>
            </div>
            <p class="tabular-nums text-muted-foreground">
              {{ t("answerTest.scores", { score: score(f.score), vector: score(f.vectorSimilarity), keyword: score(f.keywordScore) }) }}
            </p>
            <p class="whitespace-pre-wrap break-words">{{ f.excerpt }}</p>
          </li>
        </ol>
      </div>

      <div v-if="result.rawOutput !== null" class="grid gap-1">
        <h4 class="font-medium">{{ t("answerTest.declaration") }}</h4>
        <p v-if="result.declaration === null" class="text-destructive">{{ t("answerTest.noDeclaration") }}</p>
        <p v-else-if="!result.declaration.answerable">{{ t("answerTest.notAnswerable") }}</p>
        <p v-else>{{ t("answerTest.answerable", { sources: result.declaration.sources.map((n) => `[${n}]`).join(" ") }) }}</p>
        <h4 class="mt-2 font-medium">{{ t("answerTest.rawOutput") }}</h4>
        <pre class="max-h-64 overflow-auto whitespace-pre-wrap break-words rounded-md bg-muted p-2 font-mono text-xs">{{ result.rawOutput }}</pre>
      </div>

      <div v-if="result.result" class="grid gap-2">
        <h4 class="flex items-center gap-2 font-medium">
          {{ t("answerTest.final") }}
          <Badge :variant="result.result.status === 'ANSWERED' ? 'default' : 'destructive'">{{ t(`answerTest.status.${result.result.status}`) }}</Badge>
          <Badge v-if="result.result.truncated" variant="outline">{{ t("answerTest.truncated") }}</Badge>
        </h4>
        <p class="whitespace-pre-wrap break-words rounded-md border p-3">{{ result.result.text }}</p>
        <ul v-if="result.result.sources.length > 0" class="grid gap-1">
          <li v-for="s in result.result.sources" :key="s.citation" class="flex flex-wrap gap-2">
            <span>[{{ s.citation }}]</span>
            <NuxtLink :to="`/w/${workspaceId}/entries/${s.entryId}`" class="text-primary underline">{{ s.breadcrumb.join(" › ") || s.entryId }}</NuxtLink>
            <Badge v-if="s.reviewExpired" variant="outline">{{ t("answerTest.reviewExpired") }}</Badge>
          </li>
        </ul>
      </div>
    </div>
  </section>
</template>
