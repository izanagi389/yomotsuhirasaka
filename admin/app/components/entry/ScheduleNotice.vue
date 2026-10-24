<script setup lang="ts">
// 予約公開の状態（03-content-and-editing.md「予約公開（Q78）」、Q133）。記事の画面の上に出す。
// - 予約中: 時刻と予約した人。下書きが予約版と食い違っていれば知らせる（予約版は固定。差し替えは予約し直す）
// - 公開を待っている（やり直し中）: 理由と次にやり直す時刻
// - 最後の予約が、訂正による取り消しか失敗なら知らせる
// 予約し直し・取り消しは公開できる人だけ（照合はサーバー）。
import { Button } from "@/components/ui/button";
import { type EntrySchedule, personLabel } from "~/utils/content";

const props = defineProps<{ workspaceId: string; entryId: string; canPublish: boolean }>();
const emit = defineEmits<{ reschedule: [] }>();
const { t, te, locale } = useI18n();
const api = useApi();
const path = computed(() => `/workspaces/${props.workspaceId}/entries/${props.entryId}/schedule`);
const { data, error, refresh } = useAsyncData(`schedule:${props.entryId}`, () => api.get<EntrySchedule>(path.value));
const open = computed(() => data.value?.open ?? null);
/** 知らせる閉じた予約（訂正による取り消しか失敗）。 */
const notice = computed(() => {
  const l = data.value?.latest;
  return !open.value && l && (l.state === "failed" || (l.state === "cancelled" && l.reason === "corrected")) ? l : null;
});
const errorText = (code: string | null) => (code && te(`schedule.errors.${code}`) ? t(`schedule.errors.${code}`) : (code ?? ""));

const busy = ref(false);
const actionError = ref<unknown>(null);
async function cancel() {
  busy.value = true;
  actionError.value = null;
  try {
    await api.del(path.value);
    await refresh();
  } catch (e) {
    actionError.value = e;
  } finally {
    busy.value = false;
  }
}
defineExpose({ refresh });
</script>

<template>
  <div v-if="open || notice || error || actionError" class="grid gap-2">
    <FormError :error="error ?? actionError" />
    <div v-if="open" class="flex flex-wrap items-center gap-3 rounded-md border border-sky-500/50 bg-sky-500/10 p-3 text-sm" role="status" data-testid="schedule-open">
      <span class="flex-1">
        {{ t("schedule.pending", { at: formatDateTime(open.publishAt, locale), who: personLabel(open.scheduledByPerson, t("review.otherMember")) }) }}
        <template v-if="open.draftChanged">
          <br /><span class="text-amber-700 dark:text-amber-400" data-testid="schedule-draft-changed">{{ t("schedule.draftChanged") }}</span>
        </template>
        <template v-if="open.lastError">
          <br /><span class="text-destructive" data-testid="schedule-retrying">
            {{ t("schedule.retrying", { reason: errorText(open.lastError), at: formatDateTime(open.nextAttemptAt ?? "", locale) }) }}
          </span>
        </template>
      </span>
      <template v-if="canPublish">
        <Button size="sm" variant="outline" :disabled="busy" data-testid="schedule-reschedule" @click="emit('reschedule')">{{ t("schedule.reschedule") }}</Button>
        <Button size="sm" variant="outline" :disabled="busy" data-testid="schedule-cancel" @click="cancel">{{ t("schedule.cancel") }}</Button>
      </template>
    </div>
    <p v-else-if="notice" class="rounded-md border border-amber-500/50 bg-amber-500/10 p-3 text-sm" role="status" data-testid="schedule-notice">
      {{
        notice.state === "failed"
          ? t("schedule.failed", { at: formatDateTime(notice.publishAt, locale), reason: errorText(notice.lastError) })
          : t("schedule.cancelledByCorrection", { at: formatDateTime(notice.publishAt, locale) })
      }}
    </p>
  </div>
</template>
