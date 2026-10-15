<script setup lang="ts">
// 監査ログの閲覧（11-permissions-and-audit.md「監査ログ」、Q80, Q81）。読むだけ（INV-AUD-02）。
// 記録には、操作の種類・操作した人・対象のID・時刻だけがある（内容は記録していない。INV-AUD-01）。
import { Button } from "@/components/ui/button";
import type { AuditEntry, AuditPage } from "~/utils/admin";

const props = defineProps<{
  /** ワークスペースの記録。省略するとインスタンス全体（サーバー管理者だけ）。 */
  workspaceId?: string;
  /** 利用者のID → 表示名（分かる人だけ）。 */
  people?: Record<string, string>;
}>();
const { t, te, locale } = useI18n();
const api = useApi();
const path = computed(() => (props.workspaceId ? `/workspaces/${props.workspaceId}/audit-log` : "/audit-log"));

const entries = ref<AuditEntry[]>([]);
const next = ref<string | null>(null);
const loading = ref(false);
const error = ref<unknown>(null);
async function load(before?: string) {
  loading.value = true;
  error.value = null;
  try {
    const page = await api.get<AuditPage>(`${path.value}?limit=100${before ? `&before=${encodeURIComponent(before)}` : ""}`);
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
