<script setup lang="ts">
// CLI 用の個人トークン（Q91, Q119）。有効な権限は、トークンの範囲と、その時点の本人のロールの重なりだけ。
// 使えるワークスペースを選んで発行する。秘密の値は発行の応答でだけ受け取り、一度だけ表示する。発行には再認証が要る。
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import type { WorkspaceInfo } from "~/utils/content";

interface TokenSummary {
  id: string;
  name: string;
  displayPrefix: string;
  scopes: string[];
  workspaceIds: string[];
  createdAt: string;
  expiresAt: string;
  lastUsedAt: string | null;
}

const { t, locale } = useI18n();
const api = useApi();
const { me } = useAuth();
const { data: tokens, error, refresh } = await useAsyncData("me:tokens", () => api.get<TokenSummary[]>("/me/tokens"));
const { data: workspaces } = await useAsyncData("workspaces", () => api.get<WorkspaceInfo[]>("/workspaces"));
// 選べるのは所属しているワークスペースだけ（トークンでは、サーバー管理者としての全体の権限は使えない）
const memberOf = computed(() => (workspaces.value ?? []).filter((w) => w.role !== null));
const workspaceName = (id: string) => workspaces.value?.find((w) => w.id === id)?.name ?? id;

// デプロイの権限はサーバー管理者だけ
const scopeOptions = computed(() => (me.value?.isServerAdmin ? ["read", "write", "deploy"] : ["read", "write"]));
const name = ref("");
const scopes = ref<string[]>(["read"]);
const chosen = ref<string[]>([]);
const days = ref(90);
const busy = ref(false);
const actionError = ref<unknown>(null);
const created = ref<{ token: string; name: string } | null>(null);
const toggled = (list: string[], v: string, on: boolean) => (on ? [...new Set([...list, v])] : list.filter((x) => x !== v));
const toggleScope = (v: string, on: boolean) => (scopes.value = toggled(scopes.value, v, on));
const toggleWorkspace = (v: string, on: boolean) => (chosen.value = toggled(chosen.value, v, on));
async function create() {
  busy.value = true;
  actionError.value = null;
  created.value = null;
  try {
    const expiresAt = new Date(Date.now() + days.value * 86400_000).toISOString().replace(/\.\d{3}Z$/, "Z");
    created.value = await api.post<{ token: string; name: string }>("/me/tokens", {
      name: name.value.trim() || undefined,
      scopes: scopes.value,
      workspaceIds: chosen.value,
      expiresAt,
    });
    name.value = "";
    await refresh();
  } catch (e) {
    actionError.value = e;
  } finally {
    busy.value = false;
  }
}

const revoking = ref<TokenSummary | null>(null);
const revokeError = ref<unknown>(null);
async function revoke() {
  if (!revoking.value) return;
  busy.value = true;
  revokeError.value = null;
  try {
    await api.del(`/me/tokens/${revoking.value.id}`);
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
  <div class="grid max-w-3xl gap-6 text-sm">
    <h1 class="text-2xl font-semibold">{{ t("security.title") }}</h1>
    <AccountNav />
    <p class="text-muted-foreground">{{ t("account.tokens.help") }}</p>
    <form class="grid gap-3 rounded-md border p-4" @submit.prevent="create">
      <div class="grid gap-3 sm:grid-cols-2">
        <div class="grid gap-2">
          <Label for="token-name">{{ t("apiKeys.name") }}</Label>
          <Input id="token-name" v-model="name" autocomplete="off" />
        </div>
        <div class="grid gap-2">
          <Label for="token-days">{{ t("account.tokens.days") }}</Label>
          <Input id="token-days" v-model.number="days" type="number" min="1" max="365" />
        </div>
      </div>
      <fieldset class="flex flex-wrap gap-4">
        <legend class="mb-1 font-medium">{{ t("apiKeys.scopes") }}</legend>
        <label v-for="s in scopeOptions" :key="s" class="flex items-center gap-2">
          <input type="checkbox" :checked="scopes.includes(s)" @change="toggleScope(s, ($event.target as HTMLInputElement).checked)" />
          {{ t(`account.tokens.scope.${s}`) }}
        </label>
      </fieldset>
      <fieldset class="grid gap-1">
        <legend class="mb-1 font-medium">{{ t("account.tokens.workspaces") }}</legend>
        <p v-if="memberOf.length === 0" class="text-muted-foreground">{{ t("account.tokens.noMemberships") }}</p>
        <label v-for="w in memberOf" :key="w.id" class="flex items-center gap-2">
          <input type="checkbox" :checked="chosen.includes(w.id)" @change="toggleWorkspace(w.id, ($event.target as HTMLInputElement).checked)" />
          {{ w.name }}
        </label>
      </fieldset>
      <Button type="submit" class="w-fit" :disabled="busy || scopes.length === 0 || days < 1 || days > 365">{{ t("account.tokens.create") }}</Button>
    </form>
    <FormError :error="error ?? actionError" />
    <SecretOnce v-if="created" :value="created.token" :label="t('account.tokens.created', { name: created.name })" @dismiss="created = null" />

    <p v-if="tokens && tokens.length === 0" class="text-muted-foreground">{{ t("account.tokens.none") }}</p>
    <ul class="grid gap-2">
      <li v-for="k in tokens ?? []" :key="k.id" class="flex flex-wrap items-center gap-x-4 gap-y-1 rounded-md border p-3">
        <span class="font-medium">{{ k.name }}</span>
        <code class="text-xs text-muted-foreground">{{ k.displayPrefix }}…</code>
        <span>{{ k.scopes.map((s) => t(`account.tokens.scope.${s}`)).join(", ") }}</span>
        <span class="text-muted-foreground">{{ k.workspaceIds.map(workspaceName).join(", ") || t("account.tokens.noWorkspaces") }}</span>
        <span class="text-muted-foreground">{{ t("apiKeys.expiresAt", { at: formatDateTime(k.expiresAt, locale) }) }}</span>
        <Button size="sm" variant="outline" class="ml-auto" @click="revoking = k">{{ t("apiKeys.revoke") }}</Button>
      </li>
    </ul>
    <ConfirmDialog
      :open="revoking !== null"
      :title="t('account.tokens.revokeTitle')"
      :description="t('account.tokens.revokeDescription', { name: revoking?.name ?? '' })"
      :confirm-label="t('apiKeys.revoke')"
      destructive
      :busy="busy"
      :error="revokeError"
      @update:open="(v) => !v && (revoking = null)"
      @confirm="revoke"
    />
  </div>
</template>
