<script setup lang="ts">
// ワークスペースの管理: 設定（編集の引き継ぎ・プレビュー・質問ログ・関連記事の再計算）と、一番下に「危険な操作」（削除。サーバー管理者だけ）。
import QuestionLogSettingsPanel from "~/components/admin/QuestionLogSettingsPanel.vue";
import RelatedRecomputePanel from "~/components/admin/RelatedRecomputePanel.vue";
import WorkspaceDeletePanel from "~/components/admin/WorkspaceDeletePanel.vue";
import WorkspaceSettingsPanel from "~/components/admin/WorkspaceSettingsPanel.vue";

const ws = useRoute().params.ws as string;
const { t } = useI18n();
const { me } = useAuth();
const { workspace } = await useWorkspace(ws);
</script>

<template>
  <div class="grid gap-8">
    <PageHeader :title="t('adminTabs.settings')" :description="workspace?.name" />
    <WorkspaceSettingsPanel :workspace-id="ws" />
    <QuestionLogSettingsPanel :workspace-id="ws" />
    <RelatedRecomputePanel :workspace-id="ws" />
    <WorkspaceDeletePanel v-if="me?.isServerAdmin && workspace" :workspace-id="ws" :name="workspace.name" />
  </div>
</template>
