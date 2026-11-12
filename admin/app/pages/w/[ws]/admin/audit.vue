<script setup lang="ts">
// ワークスペースの管理: 監査ログと、その書き出し先（管理者とサーバー管理者）。
import { Button } from "@/components/ui/button";
import AuditPanel from "~/components/admin/AuditPanel.vue";
import AuditSinksPanel from "~/components/admin/AuditSinksPanel.vue";
import type { MemberInfo } from "~/utils/admin";

const ws = useRoute().params.ws as string;
const { t } = useI18n();
const api = useApi();
const { workspace } = await useWorkspace(ws);

// 監査ログに、操作した人のメールアドレスを出す（所属者の一覧から分かる人だけ）
const people = ref<Record<string, string>>({});
onMounted(async () => {
  try {
    const members = await api.get<MemberInfo[]>(`/workspaces/${ws}/members`);
    people.value = Object.fromEntries(members.map((m) => [m.userId, m.email]));
  } catch {
    // 分からなければIDのまま
  }
});

const showSinks = ref(false);
</script>

<template>
  <div class="grid gap-6">
    <PageHeader :title="t('adminTabs.audit')" :description="workspace?.name">
      <template #actions>
        <Button variant="outline" :aria-expanded="showSinks" data-testid="audit-sinks-toggle" @click="showSinks = !showSinks">{{ t("auditSinks.toggle") }}</Button>
      </template>
    </PageHeader>
    <AuditSinksPanel v-if="showSinks" :workspace-id="ws" />
    <AuditPanel :workspace-id="ws" :people="people" />
  </div>
</template>
