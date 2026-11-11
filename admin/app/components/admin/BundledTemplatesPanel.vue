<script setup lang="ts">
// 同梱のテンプレート（13-security.md「デプロイ」の同梱のテンプレート）。サーバー管理者だけが使う。
// - リリースに同梱した見本のテンプレートを、CLI なしでデプロイする。中身はサーバーに埋め込んだもので、画面からは送れない
// - まず試行（書き込まない）で、破壊的な変更と壊れるクエリを見せ、確認してからデプロイする
// - デプロイは再認証が要る（サーバーが判断し、useApi が再認証のダイアログを出す）
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";

interface Bundled {
  name: string;
  deployedVersion: number | null;
  upToDate: boolean;
  siteUrl: string | null;
  workspaces: number;
}
interface Check {
  dryRun: true;
  problems: string[];
  destructive: string[];
  brokenQueries: { workspaceId: string; queryId: string; problems: string[] }[];
  unchanged: boolean;
}
interface Outcome {
  version: number;
  unchanged: boolean;
  workspaces: { workspaceId: string; state: "ok" | "pending"; error?: string }[];
}

const { t } = useI18n();
const api = useApi();
const { data, error, refresh } = await useAsyncData("server:bundled-templates", () => api.get<Bundled[]>("/bundled-templates"));
// 記事の公開URLのサイト。見本の公開URLのひな形の先頭（https://example.com）を、これに置き換えてデプロイする
const siteUrl = ref(data.value?.find((b) => b.siteUrl)?.siteUrl ?? "");

const checking = ref<string | null>(null);
const check = ref<{ name: string; result: Check } | null>(null);
const checkError = ref<unknown>(null);
async function runCheck(name: string) {
  checking.value = name;
  checkError.value = null;
  check.value = null;
  done.value = null;
  try {
    check.value = { name, result: await api.post<Check>(`/bundled-templates/${name}/deploy`, { siteUrl: siteUrl.value.trim(), dryRun: true }) };
  } catch (e) {
    checkError.value = e;
  } finally {
    checking.value = null;
  }
}

const allowDestructive = ref(false);
const allowBrokenQueries = ref(false);
const confirming = ref(false);
const busy = ref(false);
const deployError = ref<unknown>(null);
const done = ref<{ name: string; outcome: Outcome } | null>(null);
const blocked = computed(() => {
  const r = check.value?.result;
  if (!r) return true;
  return r.problems.length > 0 || (r.destructive.length > 0 && !allowDestructive.value);
});
async function deploy() {
  if (!check.value) return;
  busy.value = true;
  deployError.value = null;
  try {
    const outcome = await api.post<Outcome>(`/bundled-templates/${check.value.name}/deploy`, {
      siteUrl: siteUrl.value.trim(),
      allowDestructive: allowDestructive.value,
      allowBrokenQueries: allowBrokenQueries.value,
    });
    done.value = { name: check.value.name, outcome };
    check.value = null;
    confirming.value = false;
    await refresh();
  } catch (e) {
    deployError.value = e;
  } finally {
    busy.value = false;
  }
}
</script>

<template>
  <div class="grid gap-4" data-testid="bundled-templates">
    <p class="text-muted-foreground">{{ t("bundledTemplates.help") }}</p>
    <div class="grid max-w-md gap-2">
      <Label for="bundled-site-url">{{ t("bundledTemplates.siteUrl") }}</Label>
      <Input id="bundled-site-url" v-model="siteUrl" type="url" autocomplete="off" placeholder="https://example.com" aria-describedby="bundled-site-url-help" @input="check = null" />
      <p id="bundled-site-url-help" class="text-xs text-muted-foreground">{{ t("bundledTemplates.siteUrlHelp") }}</p>
    </div>
    <FormError :error="error ?? checkError" />
    <ul class="grid gap-2">
      <li v-for="b in data ?? []" :key="b.name" class="flex flex-wrap items-center gap-2 rounded-md border p-3">
        <span class="font-medium">{{ b.name }}</span>
        <Badge v-if="b.deployedVersion === null" variant="outline">{{ t("bundledTemplates.notDeployed") }}</Badge>
        <Badge v-else-if="b.upToDate" variant="secondary">{{ t("bundledTemplates.upToDate", { version: b.deployedVersion }) }}</Badge>
        <Badge v-else variant="destructive">{{ t("bundledTemplates.differs", { version: b.deployedVersion }) }}</Badge>
        <span class="text-muted-foreground">{{ t("bundledTemplates.workspaces", { n: b.workspaces }) }}</span>
        <span v-if="b.siteUrl" class="text-muted-foreground">{{ b.siteUrl }}</span>
        <Button class="ml-auto" size="sm" variant="outline" :disabled="checking !== null || !siteUrl.trim()" @click="runCheck(b.name)">
          {{ t("bundledTemplates.check") }}
        </Button>
      </li>
    </ul>

    <section v-if="check" class="grid gap-3 rounded-md border p-4" data-testid="bundled-check">
      <h2 class="font-medium">{{ t("bundledTemplates.checkTitle", { name: check.name }) }}</h2>
      <p class="text-muted-foreground">{{ t("bundledTemplates.siteUrlUsed", { url: siteUrl.trim() }) }}</p>
      <p v-if="check.result.unchanged">{{ t("bundledTemplates.unchanged") }}</p>
      <div v-if="check.result.problems.length > 0">
        <p class="text-destructive">{{ t("bundledTemplates.problems") }}</p>
        <ul class="list-disc pl-5">
          <li v-for="p in check.result.problems" :key="p">{{ p }}</li>
        </ul>
      </div>
      <div v-if="check.result.destructive.length > 0" class="grid gap-1">
        <p class="text-destructive">{{ t("bundledTemplates.destructive") }}</p>
        <ul class="list-disc pl-5">
          <li v-for="d in check.result.destructive" :key="d">{{ d }}</li>
        </ul>
        <label class="flex items-center gap-2"><input v-model="allowDestructive" type="checkbox" />{{ t("bundledTemplates.allowDestructive") }}</label>
      </div>
      <div v-if="check.result.brokenQueries.length > 0" class="grid gap-1">
        <p class="text-destructive">{{ t("bundledTemplates.brokenQueries") }}</p>
        <ul class="list-disc pl-5">
          <li v-for="q in check.result.brokenQueries" :key="q.workspaceId + q.queryId">{{ q.workspaceId }}: {{ q.queryId }}</li>
        </ul>
        <label class="flex items-center gap-2"><input v-model="allowBrokenQueries" type="checkbox" />{{ t("bundledTemplates.allowBrokenQueries") }}</label>
      </div>
      <p v-if="!check.result.unchanged && check.result.problems.length === 0 && check.result.destructive.length === 0 && check.result.brokenQueries.length === 0">
        {{ t("bundledTemplates.ready") }}
      </p>
      <div>
        <Button :disabled="blocked" data-testid="bundled-deploy" @click="confirming = true">{{ t("bundledTemplates.deploy") }}</Button>
      </div>
    </section>

    <section v-if="done" class="grid gap-1 rounded-md border p-4" role="status">
      <p>{{ t(done.outcome.unchanged ? "bundledTemplates.doneUnchanged" : "bundledTemplates.done", { name: done.name, version: done.outcome.version }) }}</p>
      <ul class="list-disc pl-5">
        <li v-for="w in done.outcome.workspaces" :key="w.workspaceId">
          {{ w.workspaceId }}: {{ w.state === "ok" ? t("bundledTemplates.migrated") : t("bundledTemplates.pending", { reason: w.error ?? "" }) }}
        </li>
      </ul>
    </section>

    <ConfirmDialog
      v-model:open="confirming"
      :title="t('bundledTemplates.confirmTitle', { name: check?.name ?? '' })"
      :description="t('bundledTemplates.confirmDescription')"
      :confirm-label="t('bundledTemplates.deploy')"
      :busy="busy"
      :error="deployError"
      @confirm="deploy"
    />
  </div>
</template>
