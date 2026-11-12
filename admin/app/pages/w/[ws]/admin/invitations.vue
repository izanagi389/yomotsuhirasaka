<script setup lang="ts">
// ワークスペースの管理: invitations（管理者とサーバー管理者。入れるかの確認は親の admin.vue が行う）。
import InvitationsPanel from "~/components/admin/InvitationsPanel.vue";

const ws = useRoute().params.ws as string;
const { t } = useI18n();
const { workspace } = await useWorkspace(ws);
</script>

<template>
  <div class="grid gap-6">
    <PageHeader :title="t('adminTabs.invitations')" :description="workspace?.name" />
    <InvitationsPanel :workspace-id="ws" />
  </div>
</template>
