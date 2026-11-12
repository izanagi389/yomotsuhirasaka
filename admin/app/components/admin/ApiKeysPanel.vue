<script setup lang="ts">
// 配信APIのキー（Q13, Q93, Q153）。秘密の値は発行の応答でだけ受け取り、この画面で一度だけ表示する（INV-KEY-01）。
// 発行には再認証が要る。用途（ビルド用・サーバー用）は回数制限の既定値だけを変える。
// 回答API（answer。M4）のスコープは、LLMの料金がかかるので read と分け、選んだときに注意を出す。
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import type { ApiKeyCreated, ApiKeySummary } from "~/utils/admin";

const props = defineProps<{ workspaceId: string }>();
const { t, locale } = useI18n();
const api = useApi();
const base = computed(() => `/workspaces/${props.workspaceId}/api-keys`);
const { data: keys, error, refresh } = useAsyncData(`api-keys:${props.workspaceId}`, () => api.get<ApiKeySummary[]>(base.value));

const name = ref("");
const scopes = ref<string[]>(["read"]);
const purpose = ref<"build" | "server">("build");
const expires = ref("");
const busy = ref(false);
const actionError = ref<unknown>(null);
const created = ref<ApiKeyCreated | null>(null);
function toggleScope(s: string, on: boolean) {
  scopes.value = on ? [...new Set([...scopes.value, s])] : scopes.value.filter((x) => x !== s);
}
async function create() {
  busy.value = true;
  actionError.value = null;
  created.value = null;
  try {
    created.value = await api.post<ApiKeyCreated>(base.value, {
      name: name.value.trim() || undefined,
      scopes: scopes.value,
      purpose: purpose.value,
      // 日付だけを入れたら、その日の終わり（UTC）まで
      expiresAt: expires.value ? `${expires.value}T23:59:59Z` : undefined,
    });
    name.value = "";
    expires.value = "";
    await refresh();
  } catch (e) {
    actionError.value = e;
  } finally {
    busy.value = false;
  }
}

const revoking = ref<ApiKeySummary | null>(null);
const revokeError = ref<unknown>(null);
async function revoke() {
  if (!revoking.value) return;
  busy.value = true;
  revokeError.value = null;
  try {
    await api.del(`${base.value}/${revoking.value.id}`);
    revoking.value = null;
    await refresh();
  } catch (e) {
    revokeError.value = e;
  } finally {
    busy.value = false;
  }
}
</script>

<template>
  <div class="grid gap-6 text-sm">
    <form class="grid gap-3 rounded-md border p-4" @submit.prevent="create">
      <div class="grid gap-3 sm:grid-cols-3">
        <div class="grid gap-2">
          <Label for="key-name">{{ t("apiKeys.name") }}</Label>
          <Input id="key-name" v-model="name" autocomplete="off" />
        </div>
        <div class="grid gap-2">
          <Label for="key-purpose">{{ t("apiKeys.purpose") }}</Label>
          <select id="key-purpose" v-model="purpose" class="h-9 rounded-md border bg-background px-3">
            <option value="build">{{ t("apiKeys.purposes.build") }}</option>
            <option value="server">{{ t("apiKeys.purposes.server") }}</option>
          </select>
        </div>
        <div class="grid gap-2">
          <Label for="key-expires">{{ t("apiKeys.expires") }}</Label>
          <Input id="key-expires" v-model="expires" type="date" />
        </div>
      </div>
      <fieldset class="flex flex-wrap gap-4">
        <legend class="mb-1 font-medium">{{ t("apiKeys.scopes") }}</legend>
        <label v-for="s in ['read', 'register', 'answer']" :key="s" class="flex items-center gap-2">
          <input type="checkbox" :checked="scopes.includes(s)" @change="toggleScope(s, ($event.target as HTMLInputElement).checked)" />
          {{ t(`apiKeys.scope.${s}`) }}
        </label>
      </fieldset>
      <p v-if="scopes.includes('answer')" class="text-muted-foreground" role="note" data-testid="api-key-answer-note">{{ t("apiKeys.answerNote") }}</p>
      <Button type="submit" class="w-fit" :disabled="busy || scopes.length === 0">{{ t("apiKeys.create") }}</Button>
    </form>
    <FormError :error="error ?? actionError" />
    <SecretOnce v-if="created" :value="created.key" :label="t('apiKeys.created', { name: created.name })" @dismiss="created = null" />

    <p v-if="keys && keys.length === 0" class="text-muted-foreground">{{ t("apiKeys.none") }}</p>
    <ul class="grid gap-2">
      <li v-for="k in keys ?? []" :key="k.id" class="flex flex-wrap items-center gap-x-4 gap-y-1 rounded-md border p-3">
        <span class="font-medium">{{ k.name }}</span>
        <code class="text-xs text-muted-foreground">{{ k.displayPrefix }}…</code>
        <span>{{ k.scopes.map((s) => t(`apiKeys.scope.${s}`)).join(", ") }}</span>
        <span>{{ t(`apiKeys.purposes.${k.purpose}`) }}</span>
        <span class="text-muted-foreground">{{ k.expiresAt ? t("apiKeys.expiresAt", { at: formatDateTime(k.expiresAt, locale) }) : t("apiKeys.noExpiry") }}</span>
        <span class="text-muted-foreground">{{ k.lastUsedAt ? t("apiKeys.lastUsed", { at: formatDateTime(k.lastUsedAt, locale) }) : t("apiKeys.neverUsed") }}</span>
        <Button size="sm" variant="outline" class="ml-auto text-destructive hover:text-destructive" @click="revoking = k">{{ t("apiKeys.revoke") }}</Button>
      </li>
    </ul>
    <ConfirmDialog
      :open="revoking !== null"
      :title="t('apiKeys.revokeTitle')"
      :description="t('apiKeys.revokeDescription', { name: revoking?.name ?? '' })"
      :confirm-label="t('apiKeys.revoke')"
      destructive
      :busy="busy"
      :error="revokeError"
      @update:open="(v) => !v && (revoking = null)"
      @confirm="revoke"
    />
  </div>
</template>
