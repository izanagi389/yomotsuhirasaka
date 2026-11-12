<script setup lang="ts">
// ワークスペースの「…」メニュー（開く・設定・削除）。設定はそのワークスペースの管理者とサーバー管理者、
// 削除はサーバー管理者だけに出す（権限の判断はサーバー）。
import { Ellipsis, ExternalLink, Settings, Trash2 } from "@lucide/vue";

import { Button } from "@/components/ui/button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";

const props = defineProps<{ workspace: { id: string; name: string; role: string | null } }>();
const { t } = useI18n();
const { me } = useAuth();
const deleting = ref(false);
const canDelete = computed(() => !!me.value?.isServerAdmin);
</script>

<template>
  <DropdownMenu>
    <DropdownMenuTrigger as-child>
      <Button variant="ghost" size="icon" :aria-label="t('workspaces.actions', { name: props.workspace.name })" data-testid="workspace-actions">
        <Ellipsis aria-hidden="true" />
      </Button>
    </DropdownMenuTrigger>
    <DropdownMenuContent align="end" class="w-52">
      <DropdownMenuItem @select="navigateTo(`/w/${workspace.id}`)"><ExternalLink aria-hidden="true" />{{ t("workspaces.open") }}</DropdownMenuItem>
      <DropdownMenuItem v-if="isWorkspaceAdmin(workspace)" @select="navigateTo(`/w/${workspace.id}/admin?tab=settings`)">
        <Settings aria-hidden="true" />{{ t("workspaces.settings") }}
      </DropdownMenuItem>
      <template v-if="canDelete">
        <DropdownMenuSeparator />
        <DropdownMenuItem variant="destructive" data-testid="workspace-delete-menu" @select="deleting = true">
          <Trash2 aria-hidden="true" />{{ t("workspaceDelete.openDialog") }}
        </DropdownMenuItem>
      </template>
    </DropdownMenuContent>
  </DropdownMenu>
  <WorkspaceDeleteDialog v-if="canDelete" v-model:open="deleting" :workspace-id="workspace.id" :name="workspace.name" />
</template>
