<script setup lang="ts">
// ワークスペースの管理: correctionTemplates（管理者とサーバー管理者。入れるかの確認は親の admin.vue が行う）。
import CorrectionTemplatesPanel from "~/components/admin/CorrectionTemplatesPanel.vue";

const ws = useRoute().params.ws as string;
const { t } = useI18n();
const { workspace, manifest } = await useWorkspace(ws);
</script>

<template>
  <div class="grid gap-6">
    <PageHeader :title="t('adminTabs.correctionTemplates')" :description="workspace?.name" />
    <CorrectionTemplatesPanel :workspace-id="ws" :locales="manifest?.locales ?? []" />
  </div>
</template>
