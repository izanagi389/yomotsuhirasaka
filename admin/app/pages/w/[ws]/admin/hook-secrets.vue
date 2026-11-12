<script setup lang="ts">
// ワークスペースの管理: hookSecrets（管理者とサーバー管理者。入れるかの確認は親の admin.vue が行う）。
import HookSecretsPanel from "~/components/admin/HookSecretsPanel.vue";

const ws = useRoute().params.ws as string;
const { t } = useI18n();
const { workspace } = await useWorkspace(ws);
</script>

<template>
  <div class="grid gap-6">
    <PageHeader :title="t('adminTabs.hookSecrets')" :description="workspace?.name" />
    <HookSecretsPanel :workspace-id="ws" />
  </div>
</template>
