<script setup lang="ts">
// ワークスペースの設定（管理者・サーバー管理者。変更は再認証が要り、監査ログに残る）。
// - 編集の引き継ぎを誰ができるか（03-content-and-editing.md「同時編集（Q73）」）: 編集できる人なら誰でも（既定）か、
//   重い権限「編集の引き継ぎ」を持つ人だけか。権限は「所属者」のタブで付与する。
import type { WorkspaceInfo } from "~/utils/content";

const props = defineProps<{ workspaceId: string }>();
const { t } = useI18n();
const api = useApi();
const { data: workspace, error, refresh } = useAsyncData(`workspace-settings:${props.workspaceId}`, () =>
  api.get<WorkspaceInfo>(`/workspaces/${props.workspaceId}`),
);
const saving = ref(false);
const saveError = ref<unknown>(null);
async function setPolicy(policy: "anyone" | "permission") {
  saving.value = true;
  saveError.value = null;
  try {
    await api.patch(`/workspaces/${props.workspaceId}/settings`, { takeOverPolicy: policy });
    await refresh();
    await refreshNuxtData(`workspace:${props.workspaceId}`);
  } catch (e) {
    saveError.value = e;
  } finally {
    saving.value = false;
  }
}
</script>

<template>
  <div class="grid gap-4 text-sm">
    <FormError :error="error ?? saveError" />
    <fieldset v-if="workspace" class="grid gap-2" :disabled="saving" data-testid="take-over-policy">
      <legend class="mb-1 font-medium">{{ t("workspaceSettings.takeOver") }}</legend>
      <p class="text-muted-foreground">{{ t("workspaceSettings.takeOverHelp") }}</p>
      <label class="flex items-center gap-2">
        <input type="radio" name="take-over" :checked="workspace.takeOverPolicy !== 'permission'" @change="setPolicy('anyone')" />
        {{ t("workspaceSettings.anyone") }}
      </label>
      <label class="flex items-center gap-2">
        <input type="radio" name="take-over" :checked="workspace.takeOverPolicy === 'permission'" @change="setPolicy('permission')" />
        {{ t("workspaceSettings.permission") }}
      </label>
    </fieldset>
  </div>
</template>
