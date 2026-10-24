<script setup lang="ts">
// 編集中の表示（ソフトロック。03-content-and-editing.md「同時編集（Q73）」）。
// - ほかの人が編集中: 「○○さんが編集中です」と閲覧のみ。引き継げる人には「編集を引き継ぐ」
// - 自分が別の画面で編集中: その旨と、引き継ぐボタン（自分の画面からは権限なしで引き継げる）
// - 引き継がれた: 閲覧のみになったことと、保存できなかった入力は画面に残っていること
// 引き継げるかの照合はサーバーが行う（ワークスペースの設定と重い権限「編集の引き継ぎ」）。
import { Button } from "@/components/ui/button";
import type { LockHolder, LockStatus } from "~/composables/useEditLock";

const props = defineProps<{
  status: LockStatus;
  holder: LockHolder | null;
  since: string | null;
  /** 引き継げるか（表示のため。照合はサーバー）。 */
  canTakeOver: boolean;
  /** 保存できなかった入力があるか。 */
  unsaved: boolean;
  busy: boolean;
}>();
const emit = defineEmits<{ takeOver: [] }>();
const { t, locale } = useI18n();
const who = computed(() => props.holder?.name ?? t("review.otherMember"));
</script>

<template>
  <div
    v-if="status === 'held' || status === 'lost'"
    class="flex flex-wrap items-center gap-3 rounded-md border border-amber-500/50 bg-amber-500/10 p-3 text-sm"
    role="status"
    data-testid="edit-lock"
  >
    <span class="flex-1">
      <template v-if="status === 'lost'">
        {{ holder?.sameUser ? t("editLock.lostSameUser") : t("editLock.lost", { who }) }}
        <template v-if="unsaved"><br />{{ t("editLock.unsavedKept") }}</template>
      </template>
      <template v-else-if="holder?.sameUser">{{ t("editLock.sameUser") }}</template>
      <template v-else>{{ t("editLock.held", { who, at: formatDateTime(since ?? "", locale) }) }}</template>
    </span>
    <Button v-if="canTakeOver || holder?.sameUser" size="sm" variant="outline" :disabled="busy" data-testid="take-over" @click="emit('takeOver')">
      {{ t("editLock.takeOver") }}
    </Button>
  </div>
</template>
