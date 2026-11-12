<script setup lang="ts">
// ワークスペースの管理（管理者とサーバー管理者）。所属者・招待・配信APIのキー・Webhook・フックの秘密・訂正の定型文・
// テンプレート・監査ログ・設定を、それぞれ別の画面（admin/*.vue）にし、サイドバーから開く。
// ここでは、入れる人か（管理者かサーバー管理者か）だけを確かめる。権限の最終的な判断はサーバーが行う。
// 発行や変更の多くは再認証が要り、サーバーが求めたらダイアログで確認する。
const ws = useRoute().params.ws as string;
const { t } = useI18n();
const { workspace, error } = await useWorkspace(ws);
</script>

<template>
  <div class="grid gap-6">
    <FormError :error="error" />
    <p v-if="workspace && !isWorkspaceAdmin(workspace)" class="text-muted-foreground">{{ t("errors.forbidden") }}</p>
    <NuxtPage v-else-if="workspace" />
  </div>
</template>
