<script setup lang="ts">
// ワークスペースの LLM の接続設定の上書き（10-llm-and-budget.md「接続設定の単位」、Q26。M4）。
// - サーバー管理者: このワークスペースに上書きを許すか（PUT /limits の llmOverride）
// - 重い権限 manageLlm を持つ人: 接続先（サーバー管理者が許可した名前から）・APIキー・チャットのモデル・単価・埋め込みのモデル
//   APIキーは入力だけ（読み出せない。「設定済み」と日時だけを出す）。空のまま保存すれば、保存してあるキーを使い続ける
// - 埋め込みのモデルを変えたら、見積もりを見てから切り替えを開始する（そろうまで前のモデルで検索を続ける）
// 再認証・権限・値の検査はサーバーが行う（再認証のダイアログは useApi が出す）。
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import type { WorkspaceLlmView } from "~/utils/content";

const props = defineProps<{ workspaceId: string; serverAdmin: boolean }>();
const { t, locale } = useI18n();
const api = useApi();
const path = `/workspaces/${props.workspaceId}/llm`;
const { data, error, refresh } = await useAsyncData(`workspace-llm:${props.workspaceId}`, () => api.get<WorkspaceLlmView>(path));

const endpoint = ref("");
const apiKey = ref("");
const chatModel = ref("");
const inputPrice = ref("");
const outputPrice = ref("");
const embeddingModel = ref("");
watch(
  data,
  (d) => {
    const s = d?.settings;
    endpoint.value = s?.endpoint ?? d?.endpoints[0] ?? "";
    apiKey.value = "";
    chatModel.value = s?.chatModel ?? "";
    inputPrice.value = s?.chatInputPrice == null ? "" : String(s.chatInputPrice);
    outputPrice.value = s?.chatOutputPrice == null ? "" : String(s.chatOutputPrice);
    embeddingModel.value = s?.embeddingModel ?? "";
  },
  { immediate: true },
);
const price = (v: string) => (v.trim() === "" ? undefined : Number(v));
const priceProblem = computed(() =>
  [inputPrice.value, outputPrice.value].some((v) => v.trim() !== "" && !(Number(v) >= 0)) ? t("llmOverride.priceInvalid") : null,
);
// 初めてのときと、接続先を変えるときは、キーを入れ直す
const keyRequired = computed(() => apiKey.value.trim() === "" && (!data.value?.settings || data.value.settings.endpoint !== endpoint.value));

const busy = ref(false);
const saveError = ref<unknown>(null);
const saved = ref<string | null>(null);
async function run(what: string, call: () => Promise<unknown>) {
  busy.value = true;
  saveError.value = null;
  saved.value = null;
  try {
    await call();
    await refresh();
    saved.value = what;
  } catch (e) {
    saveError.value = e;
  } finally {
    busy.value = false;
  }
}
function save() {
  if (priceProblem.value || keyRequired.value) return;
  const body: Record<string, unknown> = {
    endpoint: endpoint.value,
    chatModel: chatModel.value.trim() === "" ? undefined : chatModel.value.trim(),
    chatInputPrice: price(inputPrice.value),
    chatOutputPrice: price(outputPrice.value),
    embeddingModel: embeddingModel.value === "" ? null : embeddingModel.value,
  };
  if (apiKey.value.trim() !== "") body["apiKey"] = apiKey.value;
  void run("saved", () => api.put(path, body));
}
const allow = (v: boolean) => run("allowSaved", () => api.put(`/workspaces/${props.workspaceId}/limits`, { llmOverride: v }));
const confirmingRemove = ref(false);
const remove = () =>
  run("removed", async () => {
    await api.del(path);
    confirmingRemove.value = false;
  });
const startSwitch = () => run("switchStarted", () => api.post(`${path}/embedding-switch`));
const canStart = computed(() => {
  const e = data.value?.embedding;
  return !!e && e.desiredModel !== e.activeModel && (e.state === "notStarted" || e.state === "needsMigration");
});
</script>

<template>
  <section class="grid gap-4 text-sm" data-testid="llm-override">
    <h2 class="text-lg font-semibold">{{ t("llmOverride.title") }}</h2>
    <p class="text-muted-foreground">{{ t("llmOverride.help") }}</p>
    <FormError :error="error ?? saveError" />
    <span v-if="saved" class="text-muted-foreground" role="status">{{ t(`llmOverride.${saved}`) }}</span>

    <template v-if="data">
      <!-- サーバー管理者: 上書きを許すか -->
      <div v-if="serverAdmin" class="flex items-center gap-2" data-testid="llm-override-allow">
        <Switch id="llm-override-allow" :model-value="data.allowed" :disabled="busy" @update:model-value="(v: boolean) => allow(v)" />
        <Label for="llm-override-allow">{{ t("llmOverride.allow") }}</Label>
      </div>
      <p v-if="!data.allowed" class="text-muted-foreground" data-testid="llm-override-not-allowed">{{ t("llmOverride.notAllowed") }}</p>
      <p v-else-if="data.endpoints.length === 0" class="text-muted-foreground">{{ t("llmOverride.noEndpoints") }}</p>

      <form v-if="data.allowed && data.endpoints.length > 0" class="grid gap-3" @submit.prevent="save">
        <div class="grid gap-1.5">
          <Label for="llm-endpoint">{{ t("llmOverride.endpoint") }}</Label>
          <select id="llm-endpoint" v-model="endpoint" class="h-9 max-w-xs rounded-md border bg-transparent px-2" :disabled="busy">
            <option v-for="e in data.endpoints" :key="e" :value="e">{{ e }}</option>
          </select>
        </div>
        <div class="grid gap-1.5">
          <Label for="llm-api-key">{{ t("llmOverride.apiKey") }}</Label>
          <Input id="llm-api-key" v-model="apiKey" type="password" autocomplete="off" class="max-w-md" :disabled="busy" />
          <p class="text-muted-foreground">
            {{
              !data.settings?.apiKeySet
                ? t("llmOverride.apiKeyRequired")
                : data.settings.endpoint !== endpoint
                  ? t("llmOverride.apiKeyForNewEndpoint")
                  : t("llmOverride.apiKeySet", { at: formatDateTime(data.settings.apiKeyUpdatedAt, locale) })
            }}
          </p>
        </div>
        <div class="grid gap-1.5">
          <Label for="llm-chat-model">{{ t("llmOverride.chatModel") }}</Label>
          <Input id="llm-chat-model" v-model="chatModel" class="max-w-md" :disabled="busy" :placeholder="t('llmOverride.chatModelPlaceholder')" />
        </div>
        <div class="flex flex-wrap gap-3">
          <div class="grid gap-1.5">
            <Label for="llm-input-price">{{ t("llmOverride.inputPrice") }}</Label>
            <Input id="llm-input-price" v-model="inputPrice" inputmode="decimal" class="max-w-40" :disabled="busy" />
          </div>
          <div class="grid gap-1.5">
            <Label for="llm-output-price">{{ t("llmOverride.outputPrice") }}</Label>
            <Input id="llm-output-price" v-model="outputPrice" inputmode="decimal" class="max-w-40" :disabled="busy" />
          </div>
        </div>
        <p v-if="priceProblem" class="text-destructive" role="alert">{{ priceProblem }}</p>
        <div class="grid gap-1.5">
          <Label for="llm-embedding-model">{{ t("llmOverride.embeddingModel") }}</Label>
          <select id="llm-embedding-model" v-model="embeddingModel" class="h-9 max-w-xs rounded-md border bg-transparent px-2" :disabled="busy">
            <option value="">{{ t("llmOverride.embeddingInstance") }}</option>
            <option v-for="m in data.embeddingModels" :key="m" :value="m">{{ m }}</option>
          </select>
          <p class="text-muted-foreground">{{ t("llmOverride.embeddingHelp") }}</p>
        </div>
        <p class="text-muted-foreground">{{ t("llmOverride.egressNote") }}</p>
        <div class="flex flex-wrap items-center gap-3">
          <Button type="submit" size="sm" :disabled="busy || !!priceProblem || keyRequired">{{ t("common.save") }}</Button>
          <Button v-if="data.settings" type="button" variant="outline" size="sm" :disabled="busy" @click="confirmingRemove = true">
            {{ t("llmOverride.remove") }}
          </Button>
        </div>
      </form>

      <!-- 埋め込みのモデルの切り替え -->
      <div v-if="data.embedding" class="grid gap-2 border-t pt-4" data-testid="llm-embedding-switch">
        <h3 class="font-medium">{{ t("llmOverride.embeddingSwitchTitle") }}</h3>
        <p>{{ t(`llmOverride.state.${data.embedding.state}`, { active: data.embedding.activeModel, desired: data.embedding.desiredModel }) }}</p>
        <Alert v-if="data.embedding.failedCode" variant="destructive" data-testid="llm-embedding-failed">
          <AlertDescription>{{ t("llmOverride.switchFailed", { code: data.embedding.failedCode }) }}</AlertDescription>
        </Alert>
        <template v-if="canStart || data.embedding.failedCode">
          <p class="text-muted-foreground">
            {{
              t("llmOverride.estimate", {
                chunks: data.embedding.chunksToEmbed.toLocaleString(locale),
                tokens: data.embedding.estimatedTokens.toLocaleString(locale),
              })
            }}
            <template v-if="data.embedding.estimatedCost !== null">
              {{ t("llmOverride.estimateCost", { cost: data.embedding.estimatedCost.toLocaleString(locale, { maximumFractionDigits: 4 }) }) }}
            </template>
          </p>
          <div>
            <Button size="sm" :disabled="busy" @click="startSwitch">{{ t("llmOverride.startSwitch") }}</Button>
          </div>
        </template>
      </div>
    </template>

    <ConfirmDialog
      :open="confirmingRemove"
      :title="t('llmOverride.removeTitle')"
      :description="t('llmOverride.removeDescription')"
      :confirm-label="t('llmOverride.remove')"
      destructive
      :busy="busy"
      :error="saveError"
      @update:open="(v) => !v && (confirmingRemove = false)"
      @confirm="remove"
    />
  </section>
</template>
