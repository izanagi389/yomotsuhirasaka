<script setup lang="ts">
// Webhook（01-architecture.md「Webhook」、13-security.md「外部への送信」）。
// - 送信先は https で、公開のアドレスに解決されるものだけ（内部向けはサーバーが断る。理由はエラーに出す）
// - 署名用の秘密は、登録と入れ替えの応答でだけ受け取り、この画面で一度だけ表示する
// - 入れ替えた後も、古い秘密で24時間は署名する（受け手が切り替える猶予）
// - 送信の記録には、相手の応答の状態コードだけを出す（本文は記録していない。INV-WH-04）
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import type { WebhookDelivery, WebhookSummary } from "~/utils/admin";

const props = defineProps<{ workspaceId: string }>();
const { t, te, locale } = useI18n();
const api = useApi();
const base = computed(() => `/workspaces/${props.workspaceId}/webhooks`);
const { data: hooks, error, refresh } = useAsyncData(`webhooks:${props.workspaceId}`, () => api.get<WebhookSummary[]>(base.value));

const url = ref("");
const busy = ref(false);
const actionError = ref<unknown>(null);
const secret = ref<{ label: string; value: string } | null>(null);
async function create() {
  busy.value = true;
  actionError.value = null;
  secret.value = null;
  try {
    const created = await api.post<{ id: string; url: string; secret: string }>(base.value, { url: url.value.trim() });
    secret.value = { label: t("webhooks.created", { url: created.url }), value: created.secret };
    url.value = "";
    await refresh();
  } catch (e) {
    actionError.value = e;
  } finally {
    busy.value = false;
  }
}

async function rotate(h: WebhookSummary) {
  actionError.value = null;
  secret.value = null;
  try {
    const r = await api.post<{ secret: string; previousSecretUntil: string }>(`${base.value}/${h.id}/rotate-secret`);
    secret.value = { label: t("webhooks.rotated", { url: h.url, until: formatDateTime(r.previousSecretUntil, locale.value) }), value: r.secret };
    await refresh();
  } catch (e) {
    actionError.value = e;
  }
}

// 送信の記録
const open = ref<string | null>(null);
const deliveries = ref<WebhookDelivery[]>([]);
async function loadDeliveries(id: string) {
  try {
    deliveries.value = await api.get<WebhookDelivery[]>(`${base.value}/${id}/deliveries`);
  } catch (e) {
    actionError.value = e;
  }
}
async function toggle(h: WebhookSummary) {
  open.value = open.value === h.id ? null : h.id;
  if (open.value) await loadDeliveries(h.id);
}
async function test(h: WebhookSummary) {
  actionError.value = null;
  try {
    await api.post(`${base.value}/${h.id}/test`);
    open.value = h.id;
    await loadDeliveries(h.id);
  } catch (e) {
    actionError.value = e;
  }
}

const deleting = ref<WebhookSummary | null>(null);
const deleteError = ref<unknown>(null);
async function remove() {
  if (!deleting.value) return;
  busy.value = true;
  deleteError.value = null;
  try {
    await api.del(`${base.value}/${deleting.value.id}`);
    if (open.value === deleting.value.id) open.value = null;
    deleting.value = null;
    await refresh();
  } catch (e) {
    deleteError.value = e;
  } finally {
    busy.value = false;
  }
}
const stateVariant = (s: WebhookDelivery["state"]) => (s === "delivered" ? "default" : s === "failed" ? "destructive" : "secondary");
</script>

<template>
  <div class="grid gap-6 text-sm">
    <form class="grid gap-3 rounded-md border p-4 sm:grid-cols-[1fr_auto] sm:items-end" @submit.prevent="create">
      <div class="grid gap-2">
        <Label for="webhook-url">{{ t("webhooks.url") }}</Label>
        <Input id="webhook-url" v-model="url" type="url" placeholder="https://" autocomplete="off" required />
      </div>
      <Button type="submit" :disabled="busy || !url.trim()">{{ t("webhooks.create") }}</Button>
      <p class="text-muted-foreground sm:col-span-2">{{ t("webhooks.help") }}</p>
    </form>
    <FormError :error="error ?? actionError" />
    <SecretOnce v-if="secret" :value="secret.value" :label="secret.label" @dismiss="secret = null" />

    <p v-if="hooks && hooks.length === 0" class="text-muted-foreground">{{ t("webhooks.none") }}</p>
    <ul class="grid gap-2">
      <li v-for="h in hooks ?? []" :key="h.id" class="grid gap-2 rounded-md border p-3">
        <div class="flex flex-wrap items-center gap-2">
          <span class="break-all font-medium">{{ h.url }}</span>
          <span v-if="h.previousSecretUntil" class="text-muted-foreground">{{ t("webhooks.previousSecret", { until: formatDateTime(h.previousSecretUntil, locale) }) }}</span>
          <div class="ml-auto flex flex-wrap gap-2">
            <Button size="sm" variant="outline" @click="test(h)">{{ t("webhooks.test") }}</Button>
            <Button size="sm" variant="outline" :aria-expanded="open === h.id" @click="toggle(h)">{{ t("webhooks.deliveries") }}</Button>
            <Button size="sm" variant="outline" @click="rotate(h)">{{ t("webhooks.rotate") }}</Button>
            <Button size="sm" variant="outline" class="text-destructive hover:text-destructive" @click="deleting = h">{{ t("webhooks.delete") }}</Button>
          </div>
        </div>
        <div v-if="open === h.id" class="grid gap-1">
          <div class="flex">
            <Button size="sm" variant="ghost" class="ml-auto" @click="loadDeliveries(h.id)">{{ t("webhooks.reload") }}</Button>
          </div>
          <p v-if="deliveries.length === 0" class="text-muted-foreground">{{ t("webhooks.noDeliveries") }}</p>
          <table v-else class="w-full">
            <tbody>
              <tr v-for="d in deliveries" :key="d.id" class="border-t">
                <td class="py-1 pr-3 text-muted-foreground">{{ formatDateTime(d.createdAt, locale) }}</td>
                <td class="py-1 pr-3"><code class="text-xs">{{ d.eventType }}</code></td>
                <td class="py-1 pr-3"><Badge :variant="stateVariant(d.state)">{{ t(`webhooks.state.${d.state}`) }}</Badge></td>
                <td class="py-1 pr-3">{{ t("webhooks.attempts", { n: d.attempts }) }}</td>
                <td class="py-1 text-muted-foreground">{{ d.lastStatus ?? "" }} {{ d.lastError ? (te(`webhooks.error.${d.lastError}`) ? t(`webhooks.error.${d.lastError}`) : d.lastError) : "" }}</td>
              </tr>
            </tbody>
          </table>
        </div>
      </li>
    </ul>
    <ConfirmDialog
      :open="deleting !== null"
      :title="t('webhooks.deleteTitle')"
      :description="t('webhooks.deleteDescription', { url: deleting?.url ?? '' })"
      :confirm-label="t('webhooks.delete')"
      destructive
      :busy="busy"
      :error="deleteError"
      @update:open="(v) => !v && (deleting = null)"
      @confirm="remove"
    />
  </div>
</template>
