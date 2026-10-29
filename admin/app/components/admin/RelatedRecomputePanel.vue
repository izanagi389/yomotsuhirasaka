<script setup lang="ts">
// 関連記事の一括再計算を今すぐ始める（08-search-and-related.md「計算のタイミング」）。管理者とサーバー管理者が使う。
// 毎日の一括計算とは別に数える。保存済みのベクトルを使うので、LLMの費用はかからない。
import { Button } from "@/components/ui/button";

const props = defineProps<{ workspaceId: string }>();
const { t } = useI18n();
const api = useApi();
const busy = ref(false);
const error = ref<unknown>(null);
const result = ref<"queued" | "alreadyQueued" | null>(null);
async function recompute() {
  busy.value = true;
  error.value = null;
  result.value = null;
  try {
    const res = await api.post<{ queued: boolean }>(`/workspaces/${props.workspaceId}/related/recompute`);
    result.value = res.queued ? "queued" : "alreadyQueued";
  } catch (e) {
    error.value = e;
  } finally {
    busy.value = false;
  }
}
</script>

<template>
  <section class="grid gap-2 text-sm" data-testid="related-recompute">
    <h2 class="text-lg font-semibold">{{ t("related.recomputeTitle") }}</h2>
    <p class="text-muted-foreground">{{ t("related.recomputeHelp") }}</p>
    <FormError :error="error" />
    <div class="flex flex-wrap items-center gap-3">
      <Button variant="outline" :disabled="busy" @click="recompute">{{ t("related.recompute") }}</Button>
      <span v-if="result" role="status" class="text-muted-foreground">{{ t(`related.${result}`) }}</span>
    </div>
  </section>
</template>
