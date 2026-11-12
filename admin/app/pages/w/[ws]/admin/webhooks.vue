<script setup lang="ts">
// ワークスペースの管理: webhooks（管理者とサーバー管理者。入れるかの確認は親の admin.vue が行う）。
import WebhooksPanel from "~/components/admin/WebhooksPanel.vue";

const ws = useRoute().params.ws as string;
const { t } = useI18n();
const { workspace } = await useWorkspace(ws);
</script>

<template>
  <div class="grid gap-6">
    <PageHeader :title="t('adminTabs.webhooks')" :description="workspace?.name" />
    <WebhooksPanel :workspace-id="ws" />
  </div>
</template>
