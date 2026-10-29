<script setup lang="ts">
// 埋め込みモデルの切り替え（08-search-and-related.md「モデルの変更」、Q30・Q138）。サーバー管理者だけが使う。
// - 切り替え先は設定ファイル（[llm.next_embedding]）にだけ書ける。画面では選べない
// - ワークスペースごとの状態と、作り直すチャンクの数・トークン数（単価があれば費用）の見積もりを見せ、確認してから始める
// - 開始（止まっていれば再開）は再認証が要る（サーバーが判断し、useApi が再認証のダイアログを出す）
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import type { EmbeddingSwitch, EmbeddingSwitchState } from "~/utils/content";

const { t, locale } = useI18n();
const api = useApi();
const { data, error, refresh, pending } = await useAsyncData("server:embedding-switch", () => api.get<EmbeddingSwitch>("/embedding-switch"));

const number = (n: number) => n.toLocaleString(locale.value);
const cost = (n: number) => n.toLocaleString(locale.value, { maximumFractionDigits: 4 });
const total = computed(() => {
  const rows = data.value?.workspaces ?? [];
  const costs = rows.map((w) => w.estimatedCost);
  return {
    chunks: rows.reduce((s, w) => s + w.chunksToEmbed, 0),
    tokens: rows.reduce((s, w) => s + w.estimatedTokens, 0),
    cost: costs.length > 0 && costs.every((c) => c !== null) ? costs.reduce((s, c) => s + (c ?? 0), 0) : null,
  };
});
const started = computed(() => !!data.value?.nextModel && data.value.startedModel === data.value.nextModel);
const paused = computed(() => (data.value?.workspaces ?? []).some((w) => w.state === "paused"));
const variant = (s: EmbeddingSwitchState) => (s === "needsMigration" || s === "paused" ? "destructive" : s === "switched" ? "secondary" : "outline");

const confirming = ref(false);
// 確認の画面を開いたときの「開始か再開か」（開始した直後に、閉じる途中の画面の文が変わらないように）
const resuming = ref(false);
function ask() {
  resuming.value = started.value;
  confirming.value = true;
}
const busy = ref(false);
const startError = ref<unknown>(null);
async function start() {
  busy.value = true;
  startError.value = null;
  try {
    data.value = await api.post<EmbeddingSwitch>("/embedding-switch/start");
    confirming.value = false;
  } catch (e) {
    startError.value = e;
  } finally {
    busy.value = false;
  }
}
</script>

<template>
  <div class="grid gap-4" data-testid="embedding-switch">
    <FormError :error="error" />
    <template v-if="data">
      <p v-if="!data.llmConfigured" class="text-muted-foreground">{{ t("embeddingSwitch.noLlm") }}</p>
      <template v-else>
        <dl class="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1">
          <dt class="text-muted-foreground">{{ t("embeddingSwitch.currentModel") }}</dt>
          <dd><code>{{ data.currentModel }}</code></dd>
          <dt class="text-muted-foreground">{{ t("embeddingSwitch.nextModel") }}</dt>
          <dd>
            <code v-if="data.nextModel">{{ data.nextModel }} ({{ t("embeddingSwitch.dimensions", { n: data.nextDimensions }) }})</code>
            <span v-else class="text-muted-foreground">{{ t("embeddingSwitch.noNext") }}</span>
          </dd>
        </dl>
        <Alert v-if="!data.nextModel">
          <AlertDescription>
            {{ t("embeddingSwitch.howToSet") }}
            <pre class="mt-2 overflow-x-auto rounded bg-muted p-2 text-xs">[llm.next_embedding]
model = "…"
dimensions = …</pre>
          </AlertDescription>
        </Alert>
        <div class="overflow-x-auto rounded-md border">
          <table class="w-full">
            <thead class="bg-muted/50 text-left">
              <tr>
                <th class="px-3 py-2 font-medium">{{ t("embeddingSwitch.workspace") }}</th>
                <th class="px-3 py-2 font-medium">{{ t("embeddingSwitch.state") }}</th>
                <th class="px-3 py-2 font-medium">{{ t("embeddingSwitch.activeModel") }}</th>
                <th class="px-3 py-2 text-right font-medium">{{ t("embeddingSwitch.chunks") }}</th>
                <th class="px-3 py-2 text-right font-medium">{{ t("embeddingSwitch.tokens") }}</th>
                <th class="px-3 py-2 text-right font-medium">{{ t("embeddingSwitch.cost") }}</th>
              </tr>
            </thead>
            <tbody>
              <tr v-for="w in data.workspaces" :key="w.workspaceId" class="border-t" :data-testid="`embedding-switch-${w.workspaceId}`">
                <td class="px-3 py-2">{{ w.name }}</td>
                <td class="px-3 py-2"><Badge :variant="variant(w.state)">{{ t(`embeddingSwitch.states.${w.state}`) }}</Badge></td>
                <td class="px-3 py-2"><code class="text-xs">{{ w.activeModel }}</code></td>
                <td class="px-3 py-2 text-right tabular-nums">{{ number(w.chunksToEmbed) }}</td>
                <td class="px-3 py-2 text-right tabular-nums">{{ number(w.estimatedTokens) }}</td>
                <td class="px-3 py-2 text-right tabular-nums">{{ w.estimatedCost === null ? "—" : cost(w.estimatedCost) }}</td>
              </tr>
            </tbody>
            <tfoot v-if="data.workspaces.length > 0" class="border-t bg-muted/30">
              <tr>
                <th class="px-3 py-2 text-left font-medium" colspan="3">{{ t("embeddingSwitch.total") }}</th>
                <td class="px-3 py-2 text-right tabular-nums">{{ number(total.chunks) }}</td>
                <td class="px-3 py-2 text-right tabular-nums">{{ number(total.tokens) }}</td>
                <td class="px-3 py-2 text-right tabular-nums">{{ total.cost === null ? "—" : cost(total.cost) }}</td>
              </tr>
            </tfoot>
          </table>
        </div>
        <p class="text-xs text-muted-foreground">{{ t("embeddingSwitch.estimateHelp") }}</p>
        <div v-if="data.nextModel" class="flex flex-wrap items-center gap-2">
          <Button v-if="!started || paused" data-testid="embedding-switch-start" @click="ask">
            {{ started ? t("embeddingSwitch.resume") : t("embeddingSwitch.start") }}
          </Button>
          <span v-else class="text-muted-foreground" role="status">{{ t("embeddingSwitch.running") }}</span>
          <Button variant="outline" :disabled="pending" @click="refresh()">{{ t("embeddingSwitch.reload") }}</Button>
        </div>
      </template>
    </template>
    <ConfirmDialog
      v-model:open="confirming"
      :title="resuming ? t('embeddingSwitch.confirmResumeTitle') : t('embeddingSwitch.confirmTitle', { model: data?.nextModel ?? '' })"
      :description="
        t('embeddingSwitch.confirmDescription', {
          chunks: number(total.chunks),
          tokens: number(total.tokens),
          cost: total.cost === null ? t('embeddingSwitch.noPrice') : cost(total.cost),
        })
      "
      :confirm-label="resuming ? t('embeddingSwitch.resume') : t('embeddingSwitch.start')"
      :busy="busy"
      :error="startError"
      @confirm="start"
    />
  </div>
</template>
