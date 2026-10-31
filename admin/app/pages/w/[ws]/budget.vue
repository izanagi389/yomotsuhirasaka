<script setup lang="ts">
// LLMの予算（10-llm-and-budget.md「予算」）。「埋め込みの作成」と「訪問者向けの回答」（M4。入力・出力別）。
// 重い権限 manageLlm を持つ人とサーバー管理者が使う。1つの予算の使用量・上限・天井のフォームは BudgetPartForm。
// - 天井はサーバー管理者だけが変えられる（PUT /workspaces/{ws}/limits）。ほかの人には見るだけ
// 権限・天井の範囲・再認証の判断はサーバーが行う。期間は UTC の暦の月・日で区切る。
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import BudgetPartForm from "~/components/admin/BudgetPartForm.vue";
import LlmOverridePanel from "~/components/admin/LlmOverridePanel.vue";
import type { AnswerBudget, EmbeddingBudget } from "~/utils/content";

const route = useRoute();
const ws = route.params.ws as string;
const { t, locale } = useI18n();
const api = useApi();
const { me } = useAuth();
const { workspace, error: wsError } = await useWorkspace(ws);
const allowed = computed(() => hasPermission(workspace.value, "manageLlm"));
const { data: budget, error, refresh } = await useAsyncData(`budget:${ws}`, () =>
  allowed.value ? api.get<EmbeddingBudget>(`/workspaces/${ws}/budgets/embedding`) : Promise.resolve(null),
);
const { data: answer, error: answerError, refresh: refreshAnswer } = await useAsyncData(`answer-budget:${ws}`, () =>
  allowed.value ? api.get<AnswerBudget>(`/workspaces/${ws}/budgets/answer`) : Promise.resolve(null),
);
const { refresh: refreshTodo } = useTodo(ws);

/** 保存の対象（予算の部分）と、保存できたもの。 */
type PartKey = "embedding" | "answerInput" | "answerOutput";
const saving = ref(false);
const saveError = ref<unknown>(null);
const saved = ref<{ part: PartKey; what: "limits" | "ceiling" } | null>(null);
const savedOf = (part: PartKey) => (saved.value?.part === part ? saved.value.what : null);
async function run(part: PartKey, what: "limits" | "ceiling", call: () => Promise<unknown>) {
  saving.value = true;
  saveError.value = null;
  saved.value = null;
  try {
    await call();
    await Promise.all([refresh(), refreshAnswer()]);
    await refreshTodo();
    saved.value = { part, what };
  } catch (e) {
    saveError.value = e;
  } finally {
    saving.value = false;
  }
}

type Limits = { tokensPerMonth: number | null; tokensPerDay: number | null };
type Ceiling = { month?: number | null; day?: number | null };
/** 天井の欄（`PUT /limits` の項目名）。メディアの容量の上限など、ほかの項目は送らない（送った項目だけが変わる）。 */
const CEILING_KEYS: Record<PartKey, { month: string; day: string }> = {
  embedding: { month: "embeddingTokensPerMonth", day: "embeddingTokensPerDay" },
  answerInput: { month: "answerInputTokensPerMonth", day: "answerInputTokensPerDay" },
  answerOutput: { month: "answerOutputTokensPerMonth", day: "answerOutputTokensPerDay" },
};
function saveCeiling(part: PartKey, c: Ceiling) {
  const body: Record<string, number | null> = {};
  if ("month" in c) body[CEILING_KEYS[part].month] = c.month ?? null;
  if ("day" in c) body[CEILING_KEYS[part].day] = c.day ?? null;
  return run(part, "ceiling", () => api.put(`/workspaces/${ws}/limits`, body));
}
const saveEmbedding = (l: Limits) => run("embedding", "limits", () => api.put(`/workspaces/${ws}/budgets/embedding`, l));
// 回答の上限は、変えた向きだけを送る（もう片方は変わらない）
const saveAnswer = (part: "answerInput" | "answerOutput", l: Limits) =>
  run(part, "limits", () => api.put(`/workspaces/${ws}/budgets/answer`, part === "answerInput" ? { input: l } : { output: l }));

/** 回答の予算が尽きた時刻（入力・出力の早いほう）。 */
const answerExhaustedAt = computed(() => {
  const a = answer.value;
  if (!a) return null;
  const times = [a.input.exhaustedAt, a.output.exhaustedAt].filter((x): x is string => !!x).sort();
  return times[0] ?? null;
});
</script>

<template>
  <div class="grid gap-6">
    <WorkspaceNav :workspace-id="ws" :name="workspace?.name" :admin="isWorkspaceAdmin(workspace)" :budget="allowed" :questions="hasPermission(workspace, 'viewQuestionLog')" />
    <FormError :error="wsError ?? error ?? answerError ?? saveError" />
    <p v-if="workspace && !allowed" class="text-muted-foreground">{{ t("errors.forbidden") }}</p>

    <template v-else>
      <!-- 埋め込みの作成 -->
      <section v-if="budget" class="grid gap-4" data-testid="budget-embedding">
        <h2 class="text-lg font-semibold">{{ t("budget.embeddingTitle") }}</h2>
        <p v-if="!budget.llmConfigured" class="text-sm text-muted-foreground" data-testid="budget-no-llm">{{ t("budget.noLlm") }}</p>
        <template v-else>
          <Alert v-if="budget.exhaustedAt" variant="destructive" data-testid="budget-exhausted">
            <AlertTitle>{{ t("budget.exhaustedTitle") }}</AlertTitle>
            <AlertDescription>{{ t("budget.exhaustedHelp", { at: formatDateTime(budget.exhaustedAt, locale) }) }}</AlertDescription>
          </Alert>
          <p class="text-sm text-muted-foreground">{{ t("budget.help") }}</p>
          <p class="text-sm text-muted-foreground" data-testid="budget-egress-note">{{ t("budget.egressNote") }}</p>
        </template>
        <BudgetPartForm
          :part="budget"
          id-prefix="budget"
          :configured="budget.llmConfigured"
          :server-admin="!!me?.isServerAdmin"
          :saving="saving"
          :saved="savedOf('embedding')"
          @save-limits="saveEmbedding"
          @save-ceiling="(c) => saveCeiling('embedding', c)"
        />
      </section>

      <!-- 訪問者向けの回答（M4） -->
      <section v-if="answer" class="grid gap-4 border-t pt-6" data-testid="budget-answer">
        <h2 class="text-lg font-semibold">{{ t("budget.answerTitle") }}</h2>
        <p v-if="!answer.chatConfigured" class="text-sm text-muted-foreground" data-testid="budget-no-chat">{{ t("budget.noChat") }}</p>
        <template v-else>
          <Alert v-if="answerExhaustedAt" variant="destructive" data-testid="budget-answer-exhausted">
            <AlertTitle>{{ t("budget.exhaustedTitle") }}</AlertTitle>
            <AlertDescription>{{ t("budget.answerExhaustedHelp", { at: formatDateTime(answerExhaustedAt, locale) }) }}</AlertDescription>
          </Alert>
          <p class="text-sm text-muted-foreground">{{ t("budget.answerHelp") }}</p>
          <p class="text-sm text-muted-foreground" data-testid="budget-answer-egress-note">{{ t("budget.answerEgressNote") }}</p>
        </template>
        <div class="grid gap-6 md:grid-cols-2">
          <BudgetPartForm
            :part="answer.input"
            id-prefix="answer-input"
            :heading="t('budget.direction.input')"
            :configured="answer.chatConfigured"
            :server-admin="!!me?.isServerAdmin"
            :saving="saving"
            :saved="savedOf('answerInput')"
            @save-limits="(l) => saveAnswer('answerInput', l)"
            @save-ceiling="(c) => saveCeiling('answerInput', c)"
          />
          <BudgetPartForm
            :part="answer.output"
            id-prefix="answer-output"
            :heading="t('budget.direction.output')"
            :configured="answer.chatConfigured"
            :server-admin="!!me?.isServerAdmin"
            :saving="saving"
            :saved="savedOf('answerOutput')"
            @save-limits="(l) => saveAnswer('answerOutput', l)"
            @save-ceiling="(c) => saveCeiling('answerOutput', c)"
          />
        </div>
      </section>
      <p v-if="budget || answer" class="text-xs text-muted-foreground">{{ t("budget.utc") }}</p>

      <!-- LLMの接続設定の上書き（M4） -->
      <div class="border-t pt-6">
        <LlmOverridePanel :workspace-id="ws" :server-admin="!!me?.isServerAdmin" />
      </div>
    </template>
  </div>
</template>
