<script setup lang="ts">
// ワークスペースの管理: members（管理者とサーバー管理者。入れるかの確認は親の admin.vue が行う）。
import MembersPanel from "~/components/admin/MembersPanel.vue";

const ws = useRoute().params.ws as string;
const { t } = useI18n();
const { workspace } = await useWorkspace(ws);
</script>

<template>
  <div class="grid gap-6">
    <PageHeader :title="t('adminTabs.members')" :description="workspace?.name" />
    <MembersPanel :workspace-id="ws" />
  </div>
</template>
