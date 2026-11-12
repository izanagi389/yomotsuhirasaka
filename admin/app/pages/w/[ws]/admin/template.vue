<script setup lang="ts">
// ワークスペースの管理: テンプレートの切り替え。サーバー管理者と、重い権限 switchTemplate を持つ管理者だけ
// （権限の判断はサーバー。持っていない人には画面を出さない）。
import TemplateSwitchPanel from "~/components/admin/TemplateSwitchPanel.vue";

const ws = useRoute().params.ws as string;
const { t } = useI18n();
const { me } = useAuth();
const { workspace } = await useWorkspace(ws);
const allowed = computed(() => !!me.value?.isServerAdmin || hasPermission(workspace.value, "switchTemplate"));
</script>

<template>
  <div class="grid gap-6">
    <PageHeader :title="t('adminTabs.template')" :description="workspace?.name" />
    <TemplateSwitchPanel v-if="allowed" :workspace-id="ws" />
    <p v-else class="text-muted-foreground">{{ t("errors.forbidden") }}</p>
  </div>
</template>
