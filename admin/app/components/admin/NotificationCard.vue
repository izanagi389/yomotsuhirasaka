<script setup lang="ts">
// 管理者への通知の1件（13-security.md「管理者への通知」、計画の15番）。「やること」とサーバー管理者の画面で使う。
// 通知は監査ログの記録なので、操作の種類・操作した人・対象・時刻を出す。急増の通知（alert.*）は、件数・期間と、
// 件数の多いIPアドレス・利用者も出す。「確認した」は1人ずつ（記録そのものは変わらない）。
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import type { AuditEntry } from "~/utils/admin";

const props = defineProps<{
  entry: AuditEntry;
  /** 利用者のID → 表示（分かる人だけ）。 */
  people?: Record<string, string>;
  busy?: boolean;
}>();
const emit = defineEmits<{ read: [] }>();
const { t, te, locale } = useI18n();
const actionLabel = (a: string) => (te(`audit.actions.${a}`) ? t(`audit.actions.${a}`) : a);
const who = (id: string) => props.people?.[id] ?? id;
const alert = computed(() => props.entry.action.startsWith("alert."));
</script>

<template>
  <div class="grid gap-2 rounded-md border p-3 text-sm" data-testid="notification">
    <div class="flex flex-wrap items-center gap-2">
      <Badge :variant="alert ? 'destructive' : 'outline'">{{ t("notifications.badge") }}</Badge>
      <span class="font-medium">{{ actionLabel(entry.action) }}</span>
      <span class="text-muted-foreground">{{ formatDateTime(entry.createdAt, locale) }}</span>
      <Button size="sm" variant="outline" class="ml-auto" :disabled="busy" data-testid="notification-read" @click="emit('read')">
        {{ t("notifications.read") }}
      </Button>
    </div>
    <p v-if="!alert && entry.targetIds.length > 0" class="break-all text-muted-foreground">
      {{ t("notifications.targets", { targets: entry.targetIds.map(who).join(", ") }) }}
    </p>
    <template v-if="entry.details">
      <p>{{ t("notifications.spike", { count: entry.details.count, minutes: Math.round(entry.details.windowSeconds / 60) }) }}</p>
      <div class="grid gap-3 sm:grid-cols-2">
        <div v-if="entry.details.topIps.length > 0">
          <p class="text-xs text-muted-foreground">{{ t("notifications.topIps") }}</p>
          <ul class="font-mono text-xs">
            <li v-for="i in entry.details.topIps" :key="i.ip">{{ i.ip }} — {{ i.count }}</li>
          </ul>
        </div>
        <div v-if="entry.details.topUsers.length > 0">
          <p class="text-xs text-muted-foreground">{{ t("notifications.topUsers") }}</p>
          <ul class="text-xs">
            <li v-for="u in entry.details.topUsers" :key="u.userId" class="break-all">{{ u.email ?? u.userId }} — {{ u.count }}</li>
          </ul>
        </div>
      </div>
    </template>
  </div>
</template>
