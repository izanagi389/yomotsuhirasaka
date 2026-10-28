<script setup lang="ts">
// LLMの予算（10-llm-and-budget.md「予算」）。M3は「埋め込みの作成」だけ。重い権限 manageLlm を持つ人とサーバー管理者が使う。
// - 今月・今日の使用量と効く上限（ワークスペースの上限と天井の小さいほう）、単価があれば今月の費用
// - ワークスペースの上限（月・日。「上限なし」は天井がそのまま効く）。天井を超える値は入れられない
// - 天井はサーバー管理者だけが変えられる（PUT /workspaces/{ws}/limits）。ほかの人には見るだけ
// 権限・天井の範囲・再認証の判断はサーバーが行う。期間は UTC の暦の月・日で区切る。
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import type { EmbeddingBudget } from "~/utils/content";

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
const { refresh: refreshTodo } = useTodo(ws);

const formatTokens = (n: number) => n.toLocaleString(locale.value);
const percent = (used: number, limit: number) => (limit <= 0 ? 100 : Math.min(100, Math.round((used / limit) * 100)));

/** 上限の入力（月・日）。「上限なし」なら null を送る。 */
interface LimitInput {
  unlimited: boolean;
  value: string;
}
const toInput = (v: number | null | undefined): LimitInput => ({ unlimited: v === null || v === undefined, value: v === null || v === undefined ? "" : String(v) });
const parseInput = (i: LimitInput): number | null | "invalid" => {
  if (i.unlimited) return null;
  const n = Number(i.value);
  return i.value.trim() !== "" && Number.isInteger(n) && n >= 0 ? n : "invalid";
};

// ワークスペースの上限
const month = ref<LimitInput>(toInput(null));
const day = ref<LimitInput>(toInput(null));
// 天井（サーバー管理者）。「既定値」は設定ファイルの既定値に戻す
const ceilingMonth = ref<LimitInput>(toInput(null));
const ceilingDay = ref<LimitInput>(toInput(null));
watch(
  budget,
  (b) => {
    if (!b) return;
    month.value = toInput(b.tokensPerMonth);
    day.value = toInput(b.tokensPerDay);
    ceilingMonth.value = { unlimited: false, value: String(b.ceilingTokensPerMonth) };
    ceilingDay.value = { unlimited: false, value: String(b.ceilingTokensPerDay) };
  },
  { immediate: true },
);

/** 入力の誤り（形が違う、天井を超える）。 */
function limitProblem(i: LimitInput, ceiling: number): string | null {
  const v = parseInput(i);
  if (v === "invalid") return t("budget.invalidNumber");
  if (v !== null && v > ceiling) return t("budget.exceedsCeiling", { ceiling: formatTokens(ceiling) });
  return null;
}
const monthProblem = computed(() => (budget.value ? limitProblem(month.value, budget.value.ceilingTokensPerMonth) : null));
const dayProblem = computed(() => (budget.value ? limitProblem(day.value, budget.value.ceilingTokensPerDay) : null));

// 画面の行（月・日）
const usageRows = computed(() =>
  budget.value
    ? [
        { key: "month", used: budget.value.usedThisMonth, limit: budget.value.effectiveTokensPerMonth },
        { key: "day", used: budget.value.usedToday, limit: budget.value.effectiveTokensPerDay },
      ]
    : [],
);
const limitRows = computed(() =>
  budget.value
    ? [
        { key: "month", input: month.value, problem: monthProblem.value, ceiling: budget.value.ceilingTokensPerMonth },
        { key: "day", input: day.value, problem: dayProblem.value, ceiling: budget.value.ceilingTokensPerDay },
      ]
    : [],
);
const ceilingRows = computed(() => [
  { key: "month", input: ceilingMonth.value },
  { key: "day", input: ceilingDay.value },
]);

const saving = ref(false);
const saveError = ref<unknown>(null);
const saved = ref<"limits" | "ceiling" | null>(null);
async function run(what: "limits" | "ceiling", call: () => Promise<unknown>) {
  saving.value = true;
  saveError.value = null;
  saved.value = null;
  try {
    await call();
    await refresh();
    await refreshTodo();
    saved.value = what;
  } catch (e) {
    saveError.value = e;
  } finally {
    saving.value = false;
  }
}
function saveLimits() {
  if (monthProblem.value || dayProblem.value) return;
  return run("limits", () =>
    api.put(`/workspaces/${ws}/budgets/embedding`, { tokensPerMonth: parseInput(month.value), tokensPerDay: parseInput(day.value) }),
  );
}

const ceilingProblem = computed(() =>
  [ceilingMonth.value, ceilingDay.value].some((i) => parseInput(i) === "invalid") ? t("budget.invalidNumber") : null,
);
function saveCeiling() {
  if (ceilingProblem.value || !budget.value) return;
  // 変えた欄だけを送る（送った項目だけが変わる。既定値のままの欄を、今の値で固定しないため。メディアの容量の上限も変えない）。
  // 「既定値」は null
  const body: Record<string, number | null> = {};
  const changed = (i: LimitInput, current: number) => i.unlimited || parseInput(i) !== current;
  if (changed(ceilingMonth.value, budget.value.ceilingTokensPerMonth)) body.embeddingTokensPerMonth = parseInput(ceilingMonth.value) as number | null;
  if (changed(ceilingDay.value, budget.value.ceilingTokensPerDay)) body.embeddingTokensPerDay = parseInput(ceilingDay.value) as number | null;
  if (Object.keys(body).length === 0) return;
  return run("ceiling", () => api.put(`/workspaces/${ws}/limits`, body));
}
</script>

<template>
  <div class="grid gap-6">
    <WorkspaceNav :workspace-id="ws" :name="workspace?.name" :admin="isWorkspaceAdmin(workspace)" :budget="allowed" />
    <FormError :error="wsError ?? error ?? saveError" />
    <p v-if="workspace && !allowed" class="text-muted-foreground">{{ t("errors.forbidden") }}</p>

    <template v-else-if="budget">
      <h2 class="text-lg font-semibold">{{ t("budget.embeddingTitle") }}</h2>
      <p v-if="!budget.llmConfigured" class="text-sm text-muted-foreground" data-testid="budget-no-llm">{{ t("budget.noLlm") }}</p>

      <template v-else>
        <Alert v-if="budget.exhaustedAt" variant="destructive" data-testid="budget-exhausted">
          <AlertTitle>{{ t("budget.exhaustedTitle") }}</AlertTitle>
          <AlertDescription>{{ t("budget.exhaustedHelp", { at: formatDateTime(budget.exhaustedAt, locale) }) }}</AlertDescription>
        </Alert>
        <p class="text-sm text-muted-foreground">{{ t("budget.help") }}</p>
        <p class="text-sm text-muted-foreground" data-testid="budget-egress-note">{{ t("budget.egressNote") }}</p>

        <!-- 使用量 -->
        <section class="grid gap-4 text-sm" data-testid="budget-usage">
          <div v-for="row in usageRows" :key="row.key" class="grid gap-1.5">
            <div class="flex flex-wrap items-baseline gap-2">
              <span class="font-medium">{{ t(`budget.used.${row.key}`) }}</span>
              <span class="ml-auto tabular-nums">{{ t("budget.usedOf", { used: formatTokens(row.used), limit: formatTokens(row.limit) }) }}</span>
            </div>
            <div class="h-2 overflow-hidden rounded-full bg-muted" role="progressbar" :aria-valuenow="percent(row.used, row.limit)" aria-valuemin="0" aria-valuemax="100" :aria-label="t(`budget.used.${row.key}`)">
              <div class="h-full rounded-full" :class="percent(row.used, row.limit) >= 100 ? 'bg-destructive' : 'bg-primary'" :style="{ width: `${percent(row.used, row.limit)}%` }" />
            </div>
          </div>
          <p v-if="budget.costThisMonth !== null" class="text-muted-foreground" data-testid="budget-cost">
            {{ t("budget.cost", { cost: budget.costThisMonth.toLocaleString(locale, { maximumFractionDigits: 4 }), price: budget.pricePerMillionTokens }) }}
          </p>
          <p class="text-xs text-muted-foreground">{{ t("budget.utc") }}</p>
        </section>

        <!-- ワークスペースの上限 -->
        <form class="grid gap-3 text-sm" data-testid="budget-limits" @submit.prevent="saveLimits">
          <h3 class="font-medium">{{ t("budget.limitsTitle") }}</h3>
          <p class="text-muted-foreground">{{ t("budget.limitsHelp") }}</p>
          <div v-for="row in limitRows" :key="row.key" class="grid gap-1.5">
            <Label :for="`budget-${row.key}`">{{ t(`budget.limit.${row.key}`) }}</Label>
            <div class="flex flex-wrap items-center gap-3">
              <Input :id="`budget-${row.key}`" v-model="row.input.value" inputmode="numeric" class="max-w-48" :disabled="row.input.unlimited || saving" :aria-invalid="!!row.problem" />
              <label class="flex items-center gap-2">
                <input v-model="row.input.unlimited" type="checkbox" :disabled="saving" />
                {{ t("budget.unlimited") }}
              </label>
              <span class="text-muted-foreground">{{ t("budget.ceilingIs", { ceiling: formatTokens(row.ceiling) }) }}</span>
            </div>
            <p v-if="row.problem" class="text-destructive" role="alert">{{ row.problem }}</p>
          </div>
          <div class="flex items-center gap-3">
            <Button type="submit" size="sm" :disabled="saving || !!monthProblem || !!dayProblem">{{ t("common.save") }}</Button>
            <span v-if="saved === 'limits'" class="text-muted-foreground" role="status">{{ t("budget.saved") }}</span>
          </div>
        </form>
      </template>

      <!-- 天井（サーバー管理者） -->
      <form v-if="me?.isServerAdmin" class="grid gap-3 border-t pt-4 text-sm" data-testid="budget-ceiling" @submit.prevent="saveCeiling">
        <h3 class="font-medium">{{ t("budget.ceilingTitle") }}</h3>
        <p class="text-muted-foreground">{{ t("budget.ceilingHelp") }}</p>
        <div v-for="row in ceilingRows" :key="row.key" class="grid gap-1.5">
          <Label :for="`ceiling-${row.key}`">{{ t(`budget.limit.${row.key}`) }}</Label>
          <div class="flex flex-wrap items-center gap-3">
            <Input :id="`ceiling-${row.key}`" v-model="row.input.value" inputmode="numeric" class="max-w-48" :disabled="row.input.unlimited || saving" />
            <label class="flex items-center gap-2">
              <input v-model="row.input.unlimited" type="checkbox" :disabled="saving" />
              {{ t("budget.useDefault") }}
            </label>
          </div>
        </div>
        <p v-if="ceilingProblem" class="text-destructive" role="alert">{{ ceilingProblem }}</p>
        <div class="flex items-center gap-3">
          <Button type="submit" size="sm" :disabled="saving || !!ceilingProblem">{{ t("common.save") }}</Button>
          <span v-if="saved === 'ceiling'" class="text-muted-foreground" role="status">{{ t("budget.saved") }}</span>
        </div>
      </form>
    </template>
  </div>
</template>
