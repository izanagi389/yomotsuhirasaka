<script setup lang="ts">
// 記事の鮮度（06-corrections-and-freshness.md、Q46）。
// - 「確認済み」は記事ごとの操作だけで、一括の操作は作らない。メモは管理画面だけに残る（INV-FRS-01）
// - 確認期限は、スキーマの既定・記事ごとの日数・期限なしから選ぶ
// - 「確認済み」の記録は、重い権限「訂正としての公開と確認済みの記録」を持つ人（Q76）。期限の変更は、公開できる人
// - 確認期限切れなら、編集できる人に「見直しの手がかり」（AI支援。M4）を出す
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import type { FreshnessDetail } from "~/utils/content";
import StaleHintsPanel from "~/components/assist/StaleHintsPanel.vue";

const props = defineProps<{ workspaceId: string; entryId: string; editable: boolean; canVerify: boolean }>();
const { t, locale } = useI18n();
const api = useApi();
const base = computed(() => `/workspaces/${props.workspaceId}/entries/${props.entryId}`);
const { data: detail, error, refresh } = useAsyncData(`freshness:${props.entryId}`, () => api.get<FreshnessDetail>(`${base.value}/freshness`));

// 公開したことがあるか（最終確認日は、確認済みの記録か初めての公開の日時）
const published = computed(() => !!detail.value?.lastVerifiedAt);
const busy = ref(false);
const actionError = ref<unknown>(null);
async function run(action: () => Promise<unknown>) {
  busy.value = true;
  actionError.value = null;
  try {
    await action();
    await refresh();
    return true;
  } catch (e) {
    actionError.value = e;
    return false;
  } finally {
    busy.value = false;
  }
}

// 確認済み
const memo = ref("");
async function verify() {
  if (await run(() => api.post(`${base.value}/verify`, { memo: memo.value }))) memo.value = "";
}

// 確認期限
type Mode = "default" | "custom" | "disabled";
const mode = ref<Mode>("default");
const days = ref<number | string>("");
watch(
  detail,
  (d) => {
    if (!d) return;
    mode.value = d.reviewDisabled ? "disabled" : d.reviewIntervalDays !== null ? "custom" : "default";
    days.value = d.reviewIntervalDays ?? d.defaultIntervalDays ?? "";
  },
  { immediate: true },
);
function saveReview() {
  const body =
    mode.value === "disabled"
      ? { disabled: true }
      : mode.value === "custom"
        ? { intervalDays: Number(days.value), disabled: false }
        : { disabled: false };
  void run(() => api.put(`${base.value}/review`, body));
}
</script>

<template>
  <div class="grid gap-6 text-sm">
    <FormError :error="error ?? actionError" />
    <template v-if="detail">
      <dl class="grid grid-cols-[auto_1fr] gap-x-4 gap-y-2">
        <dt class="text-muted-foreground">{{ t("freshness.lastVerified") }}</dt>
        <dd>{{ detail.lastVerifiedAt ? formatDateTime(detail.lastVerifiedAt, locale) : t("freshness.never") }}</dd>
        <dt class="text-muted-foreground">{{ t("freshness.due") }}</dt>
        <dd class="flex items-center gap-2">
          {{ detail.reviewDueAt ? formatDateTime(detail.reviewDueAt, locale) : t("freshness.noDue") }}
          <Badge v-if="detail.reviewExpired" variant="destructive" data-testid="review-expired">{{ t("freshness.expired") }}</Badge>
        </dd>
      </dl>

      <StaleHintsPanel v-if="detail.reviewExpired && editable" :workspace-id="workspaceId" :entry-id="entryId" />

      <section v-if="canVerify" class="grid gap-2">
        <h3 class="font-medium">{{ t("freshness.verify") }}</h3>
        <p class="text-muted-foreground">{{ t("freshness.verifyHelp") }}</p>
        <Textarea v-model="memo" maxlength="2000" rows="2" :placeholder="t('freshness.memo')" :aria-label="t('freshness.memo')" />
        <Button class="w-fit" :disabled="busy || !published" data-testid="mark-verified" @click="verify">{{ t("freshness.verifyButton") }}</Button>
        <p v-if="!published" class="text-muted-foreground">{{ t("freshness.notPublished") }}</p>
      </section>

      <section class="grid gap-2">
        <h3 class="font-medium">{{ t("freshness.interval") }}</h3>
        <fieldset class="grid gap-2" :disabled="!editable || busy">
          <legend class="sr-only">{{ t("freshness.interval") }}</legend>
          <label class="flex items-center gap-2">
            <input v-model="mode" type="radio" value="default" />
            {{ detail.defaultIntervalDays ? t("freshness.defaultDays", { n: detail.defaultIntervalDays }) : t("freshness.defaultNone") }}
          </label>
          <label class="flex items-center gap-2">
            <input v-model="mode" type="radio" value="custom" />
            {{ t("freshness.custom") }}
          </label>
          <div v-if="mode === 'custom'" class="ml-6 flex items-center gap-2">
            <Label for="review-days" class="sr-only">{{ t("freshness.days") }}</Label>
            <Input id="review-days" v-model="days" type="number" min="1" max="3650" class="w-28" />
            <span>{{ t("freshness.days") }}</span>
          </div>
          <label class="flex items-center gap-2">
            <input v-model="mode" type="radio" value="disabled" />
            {{ t("freshness.disabled") }}
          </label>
        </fieldset>
        <Button v-if="editable" class="w-fit" variant="outline" :disabled="busy" @click="saveReview">{{ t("common.save") }}</Button>
      </section>

      <section class="grid gap-2">
        <h3 class="font-medium">{{ t("freshness.history") }}</h3>
        <p v-if="detail.history.length === 0" class="text-muted-foreground">{{ t("freshness.noHistory") }}</p>
        <ul v-else class="grid gap-2">
          <li v-for="(h, i) in detail.history" :key="i" class="grid gap-0.5 rounded-md border p-2">
            <span class="text-muted-foreground">{{ formatDateTime(h.verifiedAt, locale) }}</span>
            <span v-if="h.memo" class="whitespace-pre-line">{{ h.memo }}</span>
          </li>
        </ul>
      </section>
    </template>
  </div>
</template>
