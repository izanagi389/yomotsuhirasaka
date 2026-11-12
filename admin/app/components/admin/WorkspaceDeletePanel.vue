<script setup lang="ts">
// 設定の一番下の「危険な操作」。サーバー管理者だけに出す。削除の確認は WorkspaceDeleteDialog が行う。
import { Button } from "@/components/ui/button";

defineProps<{ workspaceId: string; name: string }>();
const { t } = useI18n();
const confirming = ref(false);
</script>

<template>
  <section class="grid gap-3 rounded-lg border border-destructive/40 p-4 text-sm" data-testid="workspace-delete">
    <h2 class="text-base font-semibold text-destructive">{{ t("dangerZone.title") }}</h2>
    <div class="flex flex-wrap items-center justify-between gap-3">
      <div class="grid gap-1">
        <span class="font-medium">{{ t("workspaceDelete.title") }}</span>
        <span class="text-muted-foreground">{{ t("workspaceDelete.summary") }}</span>
      </div>
      <Button variant="destructive" data-testid="workspace-delete-open" @click="confirming = true">{{ t("workspaceDelete.openDialog") }}</Button>
    </div>
    <WorkspaceDeleteDialog v-model:open="confirming" :workspace-id="workspaceId" :name="name" @deleted="navigateTo('/')" />
  </section>
</template>
