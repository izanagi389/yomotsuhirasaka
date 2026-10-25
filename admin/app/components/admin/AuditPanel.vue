<script setup lang="ts">
// 監査ログの閲覧（11-permissions-and-audit.md「監査ログ」、Q80, Q81）。読むだけ（INV-AUD-02）。
// 記録には、操作の種類・操作した人・対象のID・時刻だけがある（内容は記録していない。INV-AUD-01）。
// 絞り込み（操作の種類・操作した人・期間）と、絞り込んだ結果のダウンロード（1行1件の JSON、最大1万件）ができる（計画の14番）。
// 期間は画面のタイムゾーンの日付で選び、その日の始まりから、終わりの日の翌日の始まりまでを送る。
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import type { AuditEntry, AuditPage } from "~/utils/admin";

const props = defineProps<{
  /** ワークスペースの記録。省略するとインスタンス全体（サーバー管理者だけ）。 */
  workspaceId?: string;
  /** 利用者のID → 表示名（分かる人だけ）。 */
  people?: Record<string, string>;
}>();
const { t, te, tm, locale } = useI18n();
const api = useApi();
const path = computed(() => (props.workspaceId ? `/workspaces/${props.workspaceId}/audit-log` : "/audit-log"));

const entries = ref<AuditEntry[]>([]);
const next = ref<string | null>(null);
const loading = ref(false);
const error = ref<unknown>(null);
// 絞り込み
const action = ref("");
const actorId = ref("");
const from = ref("");
const to = ref("");
/** 画面の言語の文言がある操作の種類（`group.name`）。 */
const actions = computed(() =>
  Object.entries(tm("audit.actions") as Record<string, Record<string, unknown>>)
    .flatMap(([group, names]) => Object.keys(names).map((name) => `${group}.${name}`))
    .sort(),
);
/** 日付（YYYY-MM-DD）の始まりの時刻（画面のタイムゾーン）。plusDays 日後。 */
function dayStart(date: string, plusDays = 0): string {
  const [y, m, d] = date.split("-").map(Number);
  return new Date(y!, m! - 1, d! + plusDays).toISOString();
}
const filterQuery = computed(() => {
  const q = new URLSearchParams();
  if (action.value) q.set("action", action.value);
  if (actorId.value) q.set("actor", actorId.value);
  if (from.value) q.set("from", dayStart(from.value));
  if (to.value) q.set("to", dayStart(to.value, 1));
  return q;
});
const exportHref = computed(() => {
  const q = filterQuery.value.toString();
  return `/api/admin${path.value}/export${q ? `?${q}` : ""}`;
});
function applyFilter() {
  void load();
}
function clearFilter() {
  action.value = "";
  actorId.value = "";
  from.value = "";
  to.value = "";
  void load();
}

async function load(before?: string) {
  loading.value = true;
  error.value = null;
  try {
    const q = new URLSearchParams(filterQuery.value);
    q.set("limit", "100");
    if (before) q.set("before", before);
    const page = await api.get<AuditPage>(`${path.value}?${q.toString()}`);
    entries.value = before ? [...entries.value, ...page.entries] : page.entries;
    next.value = page.nextBefore;
  } catch (e) {
    error.value = e;
  } finally {
    loading.value = false;
  }
}
onMounted(() => load());

const actionLabel = (a: string) => (te(`audit.actions.${a}`) ? t(`audit.actions.${a}`) : a);
const { me } = useAuth();
const actor = (id: string | null) => (!id ? t("audit.system") : id === me.value?.id ? t("audit.you") : (props.people?.[id] ?? id));
</script>

<template>
  <div class="grid gap-3 text-sm">
    <FormError :error="error" />
    <p class="text-muted-foreground">{{ t("audit.note") }}</p>
    <form class="flex flex-wrap items-end gap-3 rounded-md border p-3" data-testid="audit-filter" @submit.prevent="applyFilter">
      <div class="grid gap-1.5">
        <Label for="audit-action">{{ t("audit.action") }}</Label>
        <select id="audit-action" v-model="action" class="h-9 max-w-64 rounded-md border bg-background px-3">
          <option value="">{{ t("audit.all") }}</option>
          <option v-for="a in actions" :key="a" :value="a">{{ actionLabel(a) }}</option>
        </select>
      </div>
      <div v-if="people && Object.keys(people).length > 0" class="grid gap-1.5">
        <Label for="audit-actor">{{ t("audit.actor") }}</Label>
        <select id="audit-actor" v-model="actorId" class="h-9 max-w-64 rounded-md border bg-background px-3">
          <option value="">{{ t("audit.all") }}</option>
          <option v-for="(label, id) in people" :key="id" :value="id">{{ label }}</option>
        </select>
      </div>
      <div class="grid gap-1.5">
        <Label for="audit-from">{{ t("audit.from") }}</Label>
        <Input id="audit-from" v-model="from" type="date" class="w-40" />
      </div>
      <div class="grid gap-1.5">
        <Label for="audit-to">{{ t("audit.to") }}</Label>
        <Input id="audit-to" v-model="to" type="date" class="w-40" />
      </div>
      <Button type="submit" size="sm" :disabled="loading">{{ t("audit.filter") }}</Button>
      <Button type="button" size="sm" variant="ghost" @click="clearFilter">{{ t("audit.clear") }}</Button>
      <a :href="exportHref" download class="ml-auto inline-flex h-8 items-center rounded-md border px-3 text-sm hover:bg-muted" data-testid="audit-export">{{ t("audit.download") }}</a>
    </form>
    <p class="text-xs text-muted-foreground">{{ t("audit.downloadHelp") }}</p>
    <div class="overflow-x-auto rounded-md border">
      <table class="w-full">
        <thead class="bg-muted/50 text-left">
          <tr>
            <th class="px-3 py-2 font-medium">{{ t("audit.at") }}</th>
            <th class="px-3 py-2 font-medium">{{ t("audit.action") }}</th>
            <th class="px-3 py-2 font-medium">{{ t("audit.actor") }}</th>
            <th class="px-3 py-2 font-medium">{{ t("audit.targets") }}</th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="e in entries" :key="e.id" class="border-t align-top">
            <td class="whitespace-nowrap px-3 py-2 text-muted-foreground">{{ formatDateTime(e.createdAt, locale) }}</td>
            <td class="px-3 py-2">{{ actionLabel(e.action) }}</td>
            <td class="break-all px-3 py-2">{{ actor(e.actorUserId) }}</td>
            <td class="px-3 py-2"><code v-for="id in e.targetIds" :key="id" class="block break-all text-xs text-muted-foreground">{{ id }}</code></td>
          </tr>
        </tbody>
      </table>
    </div>
    <p v-if="!loading && entries.length === 0" class="text-muted-foreground">{{ t("audit.none") }}</p>
    <Button v-if="next" variant="outline" class="w-fit" :disabled="loading" @click="load(next!)">{{ t("audit.more") }}</Button>
  </div>
</template>
