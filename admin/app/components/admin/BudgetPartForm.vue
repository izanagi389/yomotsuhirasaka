<script setup lang="ts">
// 1つの予算（埋め込みの作成、または回答の入力・出力）の使用量と、上限・天井のフォーム（10-llm-and-budget.md「予算」）。
// - 今月・今日の使用量と効く上限（ワークスペースの上限と天井の小さいほう）、単価があれば今月の費用
// - ワークスペースの上限（月・日。「上限なし」は天井がそのまま効く）。天井を超える値は入れられない
// - 天井はサーバー管理者だけが変えられる（serverAdmin のときだけフォームを出す）
// 保存は親が行う（送る先が予算の種類で違うため）。権限・天井の範囲・再認証の判断はサーバーが行う。
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import type { BudgetPart } from "~/utils/content";

const props = defineProps<{
  part: BudgetPart;
  /** 入力欄の id の接頭辞（画面の中で一意）。 */
  idPrefix: string;
  /** 見出し（回答の「入力」「出力」など。なければ出さない）。 */
  heading?: string;
  serverAdmin: boolean;
  /** LLMの接続先（回答なら回答の生成のモデル）があるか。なければ使用量と上限を出さず、天井だけを出す。 */
  configured: boolean;
  saving: boolean;
  /** 保存できた方（このフォームのもの）。 */
  saved: "limits" | "ceiling" | null;
}>();
const emit = defineEmits<{
  /** ワークスペースの上限（null は上限なし）。 */
  saveLimits: [limits: { tokensPerMonth: number | null; tokensPerDay: number | null }];
  /** 天井。変えた欄だけ（null は既定値）。 */
  saveCeiling: [ceiling: { month?: number | null; day?: number | null }];
}>();
const { t, locale } = useI18n();

const formatTokens = (n: number) => n.toLocaleString(locale.value);
const percent = (used: number, limit: number) => (limit <= 0 ? 100 : Math.min(100, Math.round((used / limit) * 100)));

/** 上限の入力（月・日）。「上限なし」（天井のフォームでは「既定値」）なら null を送る。 */
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

const month = ref<LimitInput>(toInput(null));
const day = ref<LimitInput>(toInput(null));
const ceilingMonth = ref<LimitInput>(toInput(null));
const ceilingDay = ref<LimitInput>(toInput(null));
watch(
  () => props.part,
  (p) => {
    month.value = toInput(p.tokensPerMonth);
    day.value = toInput(p.tokensPerDay);
    ceilingMonth.value = { unlimited: false, value: String(p.ceilingTokensPerMonth) };
    ceilingDay.value = { unlimited: false, value: String(p.ceilingTokensPerDay) };
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
const monthProblem = computed(() => limitProblem(month.value, props.part.ceilingTokensPerMonth));
const dayProblem = computed(() => limitProblem(day.value, props.part.ceilingTokensPerDay));

const usageRows = computed(() => [
  { key: "month", used: props.part.usedThisMonth, limit: props.part.effectiveTokensPerMonth },
  { key: "day", used: props.part.usedToday, limit: props.part.effectiveTokensPerDay },
]);
const limitRows = computed(() => [
  { key: "month", input: month.value, problem: monthProblem.value, ceiling: props.part.ceilingTokensPerMonth },
  { key: "day", input: day.value, problem: dayProblem.value, ceiling: props.part.ceilingTokensPerDay },
]);
const ceilingRows = computed(() => [
  { key: "month", input: ceilingMonth.value },
  { key: "day", input: ceilingDay.value },
]);

function saveLimits() {
  if (monthProblem.value || dayProblem.value) return;
  emit("saveLimits", { tokensPerMonth: parseInput(month.value) as number | null, tokensPerDay: parseInput(day.value) as number | null });
}

const ceilingProblem = computed(() =>
  [ceilingMonth.value, ceilingDay.value].some((i) => parseInput(i) === "invalid") ? t("budget.invalidNumber") : null,
);
function saveCeiling() {
  if (ceilingProblem.value) return;
  // 変えた欄だけを送る（既定値のままの欄を、今の値で固定しないため）。「既定値」は null
  const body: { month?: number | null; day?: number | null } = {};
  const changed = (i: LimitInput, current: number) => i.unlimited || parseInput(i) !== current;
  if (changed(ceilingMonth.value, props.part.ceilingTokensPerMonth)) body.month = parseInput(ceilingMonth.value) as number | null;
  if (changed(ceilingDay.value, props.part.ceilingTokensPerDay)) body.day = parseInput(ceilingDay.value) as number | null;
  if (Object.keys(body).length === 0) return;
  emit("saveCeiling", body);
}
</script>

<template>
  <div class="grid gap-4 text-sm">
    <h4 v-if="heading" class="font-medium">{{ heading }}</h4>
    <!-- 使用量 -->
    <section v-if="configured" class="grid gap-4" :data-testid="`${idPrefix}-usage`">
      <div v-for="row in usageRows" :key="row.key" class="grid gap-1.5">
        <div class="flex flex-wrap items-baseline gap-2">
          <span class="font-medium">{{ t(`budget.used.${row.key}`) }}</span>
          <span class="ml-auto tabular-nums">{{ t("budget.usedOf", { used: formatTokens(row.used), limit: formatTokens(row.limit) }) }}</span>
        </div>
        <div class="h-2 overflow-hidden rounded-full bg-muted" role="progressbar" :aria-valuenow="percent(row.used, row.limit)" aria-valuemin="0" aria-valuemax="100" :aria-label="`${heading ? `${heading} ` : ''}${t(`budget.used.${row.key}`)}`">
          <div class="h-full rounded-full" :class="percent(row.used, row.limit) >= 100 ? 'bg-destructive' : 'bg-primary'" :style="{ width: `${percent(row.used, row.limit)}%` }" />
        </div>
      </div>
      <p v-if="part.costThisMonth !== null" class="text-muted-foreground" :data-testid="`${idPrefix}-cost`">
        {{ t("budget.cost", { cost: part.costThisMonth.toLocaleString(locale, { maximumFractionDigits: 4 }), price: part.pricePerMillionTokens }) }}
      </p>
    </section>

    <!-- ワークスペースの上限 -->
    <form v-if="configured" class="grid gap-3" :data-testid="`${idPrefix}-limits`" @submit.prevent="saveLimits">
      <h4 class="font-medium">{{ t("budget.limitsTitle") }}</h4>
      <p class="text-muted-foreground">{{ t("budget.limitsHelp") }}</p>
      <div v-for="row in limitRows" :key="row.key" class="grid gap-1.5">
        <Label :for="`${idPrefix}-${row.key}`">{{ t(`budget.limit.${row.key}`) }}</Label>
        <div class="flex flex-wrap items-center gap-3">
          <Input :id="`${idPrefix}-${row.key}`" v-model="row.input.value" inputmode="numeric" class="max-w-48" :disabled="row.input.unlimited || saving" :aria-invalid="!!row.problem" />
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

    <!-- 天井（サーバー管理者） -->
    <form v-if="serverAdmin" class="grid gap-3" :class="configured ? 'border-t pt-4' : ''" :data-testid="`${idPrefix}-ceiling`" @submit.prevent="saveCeiling">
      <h4 class="font-medium">{{ t("budget.ceilingTitle") }}</h4>
      <p class="text-muted-foreground">{{ t("budget.ceilingHelp") }}</p>
      <div v-for="row in ceilingRows" :key="row.key" class="grid gap-1.5">
        <Label :for="`${idPrefix}-ceiling-${row.key}`">{{ t(`budget.limit.${row.key}`) }}</Label>
        <div class="flex flex-wrap items-center gap-3">
          <Input :id="`${idPrefix}-ceiling-${row.key}`" v-model="row.input.value" inputmode="numeric" class="max-w-48" :disabled="row.input.unlimited || saving" />
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
  </div>
</template>
