<script setup lang="ts">
// 支援の結果の状態の知らせ（動いている途中・予算切れ・失敗）。予算切れは「今月（今日）の上限に達しました」と出し、
// 支援の部分だけに出す。公開などの操作は止めない（INV-BUD-02）。
import type { AssistState } from "~/utils/content";

defineProps<{ state: AssistState; period?: "month" | "day" }>();
const { t } = useI18n();
</script>

<template>
  <p v-if="state === 'running'" class="text-muted-foreground" role="status" data-testid="assist-running">{{ t("assist.running") }}</p>
  <p v-else-if="state === 'budgetExhausted'" class="text-amber-700 dark:text-amber-400" role="status" data-testid="assist-budget-exhausted">
    {{ period === "day" ? t("assist.budgetExhaustedDay") : t("assist.budgetExhaustedMonth") }}
  </p>
  <p v-else-if="state === 'failed'" class="text-muted-foreground" role="status" data-testid="assist-failed">{{ t("assist.failed") }}</p>
</template>
