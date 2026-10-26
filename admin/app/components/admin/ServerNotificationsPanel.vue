<script setup lang="ts">
// インスタンス単位の管理者への通知（サーバー管理者だけ。計画の15番）: 急増・サーバー管理者の作成・緊急停止など。
// まだ「確認した」にしていない、直近7日の分を古い順に出す。
import type { AuditEntry } from "~/utils/admin";
import NotificationCard from "~/components/admin/NotificationCard.vue";

defineProps<{ people?: Record<string, string> }>();
const { t } = useI18n();
const api = useApi();
const { data: items, error, refresh } = useAsyncData("server:notifications", () => api.get<AuditEntry[]>("/notifications"));
const busy = ref(false);
const readError = ref<unknown>(null);
async function read(e: AuditEntry) {
  busy.value = true;
  readError.value = null;
  try {
    await api.post(`/notifications/${e.id}/read`);
    await refresh();
  } catch (err) {
    readError.value = err;
  } finally {
    busy.value = false;
  }
}
defineExpose({ count: computed(() => items.value?.length ?? 0) });
</script>

<template>
  <div class="grid gap-3 text-sm" data-testid="server-notifications">
    <p class="text-muted-foreground">{{ t("notifications.serverHelp") }}</p>
    <FormError :error="error ?? readError" />
    <p v-if="items && items.length === 0" class="text-muted-foreground">{{ t("notifications.none") }}</p>
    <NotificationCard v-for="e in items ?? []" :key="e.id" :entry="e" :people="people" :busy="busy" @read="read(e)" />
  </div>
</template>
