<script setup lang="ts">
// 編集者向けのAI支援を使うか（10-llm-and-budget.md「編集者向けのAI支援」、Q69, Q177。M4）。既定は使わない。
// 有効にすると、公開前の下書きが外部のLLMの提供会社に送られる。その注意と、機密性の高いサイトにはローカルのLLMを勧める文を出す。
// 重い権限 manageLlm を持つ人とサーバー管理者が使う。再認証・権限の判断はサーバーが行う（再認証のダイアログは useApi が出す）。
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import type { AssistSettings } from "~/utils/content";

const props = defineProps<{ workspaceId: string }>();
const { t } = useI18n();
const api = useApi();
const path = computed(() => `/workspaces/${props.workspaceId}/assist-settings`);
const { data, error } = await useAsyncData(`assist-settings:${props.workspaceId}`, () => api.get<AssistSettings>(path.value));
const saving = ref(false);
const saveError = ref<unknown>(null);
async function toggle(enabled: boolean) {
  saving.value = true;
  saveError.value = null;
  try {
    data.value = await api.put<AssistSettings>(path.value, { enabled });
  } catch (e) {
    saveError.value = e;
  } finally {
    saving.value = false;
  }
}
</script>

<template>
  <section class="grid gap-3 text-sm" data-testid="assist-settings">
    <h3 class="font-medium">{{ t("assist.settingsTitle") }}</h3>
    <p class="text-muted-foreground">{{ t("assist.settingsHelp") }}</p>
    <Alert data-testid="assist-egress-note">
      <AlertDescription>{{ t("assist.egressNote") }}</AlertDescription>
    </Alert>
    <div v-if="data" class="flex items-center gap-2">
      <Switch id="assist-enabled" :model-value="data.enabled" :disabled="saving" @update:model-value="toggle" />
      <Label for="assist-enabled">{{ t("assist.enable") }}</Label>
    </div>
    <FormError :error="error ?? saveError" />
  </section>
</template>
