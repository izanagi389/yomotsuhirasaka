<script setup lang="ts">
// 監査ログの書き出し先（ワークスペースの Webhook。11-permissions-and-audit.md「外部への書き出し」、計画の14番）。
// - 届くのは、このワークスペースの記録の、作った時点より後のものだけ（1行1件の JSON。記事の Webhook と同じ形式の署名）
// - 署名用の秘密は、作成の応答でだけ受け取り、この画面で一度だけ表示する
// - 7日間続けて送れなければ停止になる。停止したものは、再開するまで送らない（再開すると続きから送る）
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import type { AuditSink } from "~/utils/admin";

const props = defineProps<{ workspaceId: string }>();
const { t, te, locale } = useI18n();
const api = useApi();
const { confirm } = useConfirm();
const base = computed(() => `/workspaces/${props.workspaceId}/audit-sinks`);
const { data: sinks, error, refresh } = useAsyncData(`audit-sinks:${props.workspaceId}`, () => api.get<AuditSink[]>(base.value));

const url = ref("");
const busy = ref(false);
const actionError = ref<unknown>(null);
const secret = ref<{ label: string; value: string } | null>(null);
async function run(f: () => Promise<void>) {
  busy.value = true;
  actionError.value = null;
  try {
    await f();
    await refresh();
  } catch (e) {
    actionError.value = e;
  } finally {
    busy.value = false;
  }
}
const create = () =>
  run(async () => {
    secret.value = null;
    const created = await api.post<{ id: string; url: string; secret: string }>(base.value, { url: url.value.trim() });
    secret.value = { label: t("auditSinks.created", { url: created.url }), value: created.secret };
    url.value = "";
  });
const resume = (s: AuditSink) => run(() => api.post(`${base.value}/${s.id}/resume`));
async function remove(s: AuditSink) {
  const ok = await confirm({
    title: t("confirmations.auditSinkDelete.title"),
    description: t("confirmations.auditSinkDelete.description", { url: s.url }),
    confirmLabel: t("auditSinks.delete"),
    destructive: true,
  });
  if (ok) await run(() => api.del(`${base.value}/${s.id}`));
}
const errorText = (code: string) => (te(`webhooks.error.${code}`) ? t(`webhooks.error.${code}`) : code);
</script>

<template>
  <div class="grid gap-4 text-sm" data-testid="audit-sinks">
    <h3 class="font-medium">{{ t("auditSinks.title") }}</h3>
    <p class="text-muted-foreground">{{ t("auditSinks.help") }}</p>
    <form class="grid gap-3 rounded-md border p-4 sm:grid-cols-[1fr_auto] sm:items-end" @submit.prevent="create">
      <div class="grid gap-2">
        <Label for="audit-sink-url">{{ t("auditSinks.url") }}</Label>
        <Input id="audit-sink-url" v-model="url" type="url" placeholder="https://" autocomplete="off" required />
      </div>
      <Button type="submit" :disabled="busy || !url.trim()">{{ t("auditSinks.create") }}</Button>
    </form>
    <FormError :error="error ?? actionError" />
    <SecretOnce v-if="secret" :value="secret.value" :label="secret.label" @dismiss="secret = null" />
    <p v-if="sinks && sinks.length === 0" class="text-muted-foreground">{{ t("auditSinks.none") }}</p>
    <ul class="grid gap-2">
      <li v-for="s in sinks ?? []" :key="s.id" class="grid gap-1 rounded-md border p-3">
        <div class="flex flex-wrap items-center gap-2">
          <span class="break-all font-medium">{{ s.url }}</span>
          <Badge v-if="s.stoppedAt" variant="destructive">{{ t("auditSinks.stopped") }}</Badge>
          <Badge v-else-if="s.failingSince" variant="secondary">{{ t("auditSinks.failing") }}</Badge>
          <Badge v-else variant="outline">{{ t("auditSinks.ok") }}</Badge>
          <div class="ml-auto flex flex-wrap gap-2">
            <Button v-if="s.stoppedAt" size="sm" variant="outline" :disabled="busy" @click="resume(s)">{{ t("auditSinks.resume") }}</Button>
            <Button size="sm" variant="outline" class="text-destructive hover:text-destructive" :disabled="busy" @click="remove(s)">{{ t("auditSinks.delete") }}</Button>
          </div>
        </div>
        <p v-if="s.stoppedAt" class="text-muted-foreground">{{ t("auditSinks.stoppedAt", { at: formatDateTime(s.stoppedAt, locale) }) }}</p>
        <p v-if="s.failingSince" class="text-muted-foreground">
          {{ t("auditSinks.failingSince", { at: formatDateTime(s.failingSince, locale) }) }}
          <template v-if="s.lastError"> — {{ errorText(s.lastError) }}</template>
        </p>
      </li>
    </ul>
  </div>
</template>
