<script setup lang="ts">
// ワークスペースの管理: apiKeys（管理者とサーバー管理者。入れるかの確認は親の admin.vue が行う）。
import ApiKeysPanel from "~/components/admin/ApiKeysPanel.vue";

const ws = useRoute().params.ws as string;
const { t } = useI18n();
const { workspace } = await useWorkspace(ws);
</script>

<template>
  <div class="grid gap-6">
    <PageHeader :title="t('adminTabs.apiKeys')" :description="workspace?.name" />
    <ApiKeysPanel :workspace-id="ws" />
  </div>
</template>
