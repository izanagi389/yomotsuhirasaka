<script setup lang="ts">
// 質問ログの有効・無効と保存期間（09-qa-chatbot.md「質問ログ」、Q14, Q20）。ワークスペースの管理者とサーバー管理者が使う。
// - 有効にする前に、サイトのプライバシーポリシーへの記載が必要なことと、マスクの限界を示す
// - 無効にすると、残っている質問文をすぐに消す（確かめてから）
// 再認証・権限・値の検査はサーバーが行う（再認証のダイアログは useApi が出す）。
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import type { QuestionLogSettings } from "~/utils/content";

const MIN_DAYS = 7;
const MAX_DAYS = 365;

const props = defineProps<{ workspaceId: string }>();
const { t } = useI18n();
const api = useApi();
const path = `/workspaces/${props.workspaceId}/question-log/settings`;
const { data, error } = await useAsyncData(`question-log-settings:${props.workspaceId}`, () => api.get<QuestionLogSettings>(path));

const enabled = ref(false);
const days = ref("90");
watch(
  data,
  (d) => {
    if (!d) return;
    enabled.value = d.enabled;
    days.value = String(d.retentionDays);
  },
  { immediate: true },
);
const daysProblem = computed(() => {
  const n = Number(days.value);
  return Number.isInteger(n) && n >= MIN_DAYS && n <= MAX_DAYS ? null : t("questionLog.daysInvalid", { min: MIN_DAYS, max: MAX_DAYS });
});

const saving = ref(false);
const saveError = ref<unknown>(null);
const saved = ref(false);
const confirmingDisable = ref(false);
async function put(body: Partial<QuestionLogSettings>) {
  saving.value = true;
  saveError.value = null;
  saved.value = false;
  try {
    data.value = await api.put<QuestionLogSettings>(path, body);
    saved.value = true;
    confirmingDisable.value = false;
  } catch (e) {
    saveError.value = e;
  } finally {
    saving.value = false;
  }
}
function save() {
  if (daysProblem.value || !data.value) return;
  // 無効にするときは、残っている質問文を消すことを確かめてから
  if (data.value.enabled && !enabled.value) {
    confirmingDisable.value = true;
    return;
  }
  void put({ enabled: enabled.value, retentionDays: Number(days.value) });
}
</script>

<template>
  <section class="grid gap-3 text-sm" data-testid="question-log-settings">
    <h2 class="text-lg font-semibold">{{ t("questionLog.settingsTitle") }}</h2>
    <p class="text-muted-foreground">{{ t("questionLog.settingsHelp") }}</p>
    <FormError :error="error ?? saveError" />
    <form v-if="data" class="grid gap-3" @submit.prevent="save">
      <div class="flex items-center gap-2">
        <Switch id="question-log-enabled" v-model="enabled" :disabled="saving" />
        <Label for="question-log-enabled">{{ t("questionLog.enable") }}</Label>
      </div>
      <Alert v-if="enabled" data-testid="question-log-privacy">
        <AlertDescription>{{ t("questionLog.privacyNote") }}</AlertDescription>
      </Alert>
      <div class="grid gap-1.5">
        <Label for="question-log-days">{{ t("questionLog.retentionDays", { min: MIN_DAYS, max: MAX_DAYS }) }}</Label>
        <Input id="question-log-days" v-model="days" inputmode="numeric" class="max-w-32" :disabled="saving" :aria-invalid="!!daysProblem" />
        <p v-if="daysProblem" class="text-destructive" role="alert">{{ daysProblem }}</p>
      </div>
      <div class="flex items-center gap-3">
        <Button type="submit" size="sm" :disabled="saving || !!daysProblem">{{ t("common.save") }}</Button>
        <span v-if="saved" class="text-muted-foreground" role="status">{{ t("questionLog.saved") }}</span>
      </div>
    </form>
    <ConfirmDialog
      :open="confirmingDisable"
      :title="t('questionLog.disableTitle')"
      :description="t('questionLog.disableDescription')"
      :confirm-label="t('questionLog.disable')"
      destructive
      :busy="saving"
      :error="saveError"
      @update:open="(v) => !v && (confirmingDisable = false)"
      @confirm="put({ enabled: false, retentionDays: Number(days) })"
    />
  </section>
</template>
