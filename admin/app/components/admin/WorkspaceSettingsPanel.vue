<script setup lang="ts">
// ワークスペースの設定（管理者・サーバー管理者。変更は再認証が要り、監査ログに残る）。
// - 編集の引き継ぎを誰ができるか（03-content-and-editing.md「同時編集（Q73）」）: 編集できる人なら誰でも（既定）か、
//   重い権限「編集の引き継ぎ」を持つ人だけか。権限は「所属者」のタブで付与する。
// - プレビュー（03「プレビュー（Q72）」）: プレビューを許すサイトのオリジン（1行に1つ、最大10件）と、サイトのプレビュー用ルートのパス。
//   形の検査と正規化はサーバーが行う（ここでは空行を除くだけ）。
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import type { WorkspaceInfo } from "~/utils/content";

const props = defineProps<{ workspaceId: string }>();
const { t } = useI18n();
const api = useApi();
const toast = useToast();
const { data: workspace, error, refresh } = useAsyncData(`workspace-settings:${props.workspaceId}`, () =>
  api.get<WorkspaceInfo>(`/workspaces/${props.workspaceId}`),
);
const saving = ref(false);
const saveError = ref<unknown>(null);
async function save(changes: Record<string, unknown>): Promise<boolean> {
  saving.value = true;
  saveError.value = null;
  try {
    await api.patch(`/workspaces/${props.workspaceId}/settings`, changes);
    await refresh();
    await refreshNuxtData(`workspace:${props.workspaceId}`);
    toast.show(t("workspaceSettings.saved"));
    return true;
  } catch (e) {
    saveError.value = e;
    return false;
  } finally {
    saving.value = false;
  }
}
const setPolicy = (policy: "anyone" | "permission") => save({ takeOverPolicy: policy });

// プレビュー
const originsText = ref("");
const previewPath = ref("");
watch(
  workspace,
  (w) => {
    originsText.value = (w?.previewOrigins ?? []).join("\n");
    previewPath.value = w?.previewPath ?? "/_yomi/preview";
  },
  { immediate: true },
);
const savedPreview = ref(false);
async function savePreview() {
  savedPreview.value = false;
  const previewOrigins = originsText.value
    .split("\n")
    .map((l) => l.trim())
    .filter((l) => l !== "");
  savedPreview.value = await save({ previewOrigins, previewPath: previewPath.value.trim() });
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

    <form v-if="workspace" class="grid gap-3" data-testid="preview-settings" @submit.prevent="savePreview">
      <h3 class="font-medium">{{ t("workspaceSettings.preview") }}</h3>
      <p class="text-muted-foreground">{{ t("workspaceSettings.previewHelp") }}</p>
      <div class="grid gap-1.5">
        <Label for="preview-origins">{{ t("workspaceSettings.previewOrigins") }}</Label>
        <Textarea id="preview-origins" v-model="originsText" rows="3" placeholder="https://example.com" />
        <p class="text-xs text-muted-foreground">{{ t("workspaceSettings.previewOriginsHelp") }}</p>
      </div>
      <div class="grid gap-1.5">
        <Label for="preview-path">{{ t("workspaceSettings.previewPath") }}</Label>
        <Input id="preview-path" v-model="previewPath" class="max-w-sm" />
        <p class="text-xs text-muted-foreground">{{ t("workspaceSettings.previewPathHelp") }}</p>
      </div>
      <div class="flex items-center gap-3">
        <Button type="submit" size="sm" :disabled="saving">{{ t("common.save") }}</Button>
        <span v-if="savedPreview" class="text-muted-foreground" role="status">{{ t("workspaceSettings.saved") }}</span>
      </div>
    </form>
  </div>
</template>
