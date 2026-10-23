<script setup lang="ts">
// レビュー依頼（03-content-and-editing.md「レビュー（Q77）」、計画の8番）。1段階だけ: 依頼 → 差し戻し・取り下げ・公開。
// - 依頼は下書きを編集できる人。見てほしい人は任意で、公開できる人（自分を除く）から選ぶ。目印だけで、他の公開できる人も対応できる
// - 見てほしい人の変更と取り下げは、依頼した本人と公開できる人。差し戻し（コメント必須）は公開できる人
// - 公開すると、開いている依頼は「公開済み」で閉じる（公開のボタンは記事の画面の上）
// 権限の照合はサーバーが行う。ここはボタンの出し分けだけ。
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import { type EntryReview, type ReviewPerson, personLabel } from "~/utils/content";

const props = defineProps<{ workspaceId: string; entryId: string; canEdit: boolean; canPublish: boolean }>();
const { t, locale } = useI18n();
const api = useApi();
const { me } = useAuth();
const path = computed(() => `/workspaces/${props.workspaceId}/entries/${props.entryId}/review-request`);
const { data: review, error, refresh } = useAsyncData(`review:${props.entryId}`, () => api.get<EntryReview>(path.value));
const { data: candidates } = useAsyncData(`reviewers:${props.workspaceId}`, () =>
  props.canEdit ? api.get<ReviewPerson[]>(`/workspaces/${props.workspaceId}/reviewers`) : Promise.resolve([] as ReviewPerson[]),
);

const open = computed(() => review.value?.open ?? null);
const isRequester = computed(() => !!open.value?.requestedBy && open.value.requestedBy.userId === me.value?.id);
const canManage = computed(() => isRequester.value || props.canPublish);
/** 見てほしい人の候補（依頼した本人を除く）。 */
function candidatesFor(requester: string | undefined) {
  return (candidates.value ?? []).filter((p) => p.userId !== requester);
}

const reviewerId = ref("");
const comment = ref("");
const busy = ref(false);
const actionError = ref<unknown>(null);
async function run(f: () => Promise<unknown>) {
  busy.value = true;
  actionError.value = null;
  try {
    await f();
    await refresh();
    // 「要対応」と記事の見出しの表示も更新する
    await refreshNuxtData([`review-requests:${props.workspaceId}`]);
    return true;
  } catch (e) {
    actionError.value = e;
    return false;
  } finally {
    busy.value = false;
  }
}
const request = () => run(() => api.post(path.value, { reviewerId: reviewerId.value || null }));
const changeReviewer = (id: string) => run(() => api.patch(path.value, { reviewerId: id || null }));
const withdraw = () => run(() => api.post(`${path.value}/withdraw`));
async function reject() {
  if (await run(() => api.post(`${path.value}/reject`, { comment: comment.value }))) comment.value = "";
}
const stateLabel = (s: string) => t(`review.state.${s}`);
</script>

<template>
  <div class="grid gap-6 text-sm">
    <FormError :error="error ?? actionError" />
    <p class="text-muted-foreground">{{ t("review.help") }}</p>

    <section v-if="open" class="grid gap-3 rounded-md border p-3" data-testid="review-open">
      <div class="flex flex-wrap items-center gap-2">
        <Badge>{{ t("review.waiting") }}</Badge>
        <span>{{ t("review.requestedBy", { who: personLabel(open.requestedBy, t("review.otherMember")), at: formatDateTime(open.createdAt, locale) }) }}</span>
        <Badge v-if="open.changedSinceRequest" variant="outline" data-testid="review-changed">{{ t("review.changedSinceRequest") }}</Badge>
      </div>
      <div class="flex flex-wrap items-center gap-2">
        <Label for="review-reviewer">{{ t("review.reviewer") }}</Label>
        <select
          v-if="canManage"
          id="review-reviewer"
          class="h-8 rounded-md border bg-background px-2"
          :value="open.reviewer?.userId ?? ''"
          :disabled="busy"
          data-testid="review-reviewer"
          @change="changeReviewer(($event.target as HTMLSelectElement).value)"
        >
          <option value="">{{ t("review.noReviewer") }}</option>
          <option v-for="p in candidatesFor(open.requestedBy?.userId)" :key="p.userId" :value="p.userId">{{ personLabel(p, "") }}</option>
        </select>
        <span v-else id="review-reviewer">{{ open.reviewer ? personLabel(open.reviewer, "") : t("review.noReviewer") }}</span>
        <Badge v-if="open.assignedToMe" variant="secondary">{{ t("review.assignedToMe") }}</Badge>
      </div>
      <div v-if="canPublish" class="grid gap-2">
        <Label for="review-comment">{{ t("review.rejectComment") }}</Label>
        <Textarea id="review-comment" v-model="comment" maxlength="2000" rows="3" data-testid="review-comment" />
        <p class="text-muted-foreground">{{ t("review.rejectHelp") }}</p>
      </div>
      <div class="flex flex-wrap gap-2">
        <Button v-if="canPublish" variant="destructive" :disabled="busy || !comment.trim()" data-testid="review-reject" @click="reject">{{ t("review.reject") }}</Button>
        <Button v-if="canManage" variant="outline" :disabled="busy" data-testid="review-withdraw" @click="withdraw">{{ t("review.withdraw") }}</Button>
      </div>
    </section>

    <section v-else-if="canEdit" class="grid gap-2" data-testid="review-request">
      <h3 class="font-medium">{{ t("review.request") }}</h3>
      <div class="flex flex-wrap items-center gap-2">
        <Label for="review-new-reviewer">{{ t("review.reviewer") }}</Label>
        <select id="review-new-reviewer" v-model="reviewerId" class="h-8 rounded-md border bg-background px-2" :disabled="busy" data-testid="review-new-reviewer">
          <option value="">{{ t("review.noReviewer") }}</option>
          <option v-for="p in candidatesFor(me?.id)" :key="p.userId" :value="p.userId">{{ personLabel(p, "") }}</option>
        </select>
      </div>
      <Button class="w-fit" :disabled="busy" data-testid="review-submit" @click="request">{{ t("review.requestButton") }}</Button>
    </section>

    <section class="grid gap-2">
      <h3 class="font-medium">{{ t("review.history") }}</h3>
      <p v-if="review && review.history.length === 0" class="text-muted-foreground">{{ t("review.noHistory") }}</p>
      <ul v-else class="grid gap-2">
        <li v-for="r in review?.history ?? []" :key="r.id" class="grid gap-1 rounded-md border p-3" data-testid="review-history-item">
          <div class="flex flex-wrap items-center gap-2">
            <Badge :variant="r.state === 'rejected' ? 'destructive' : 'outline'">{{ stateLabel(r.state) }}</Badge>
            <span>{{ t("review.requestedBy", { who: personLabel(r.requestedBy, t("review.otherMember")), at: formatDateTime(r.createdAt, locale) }) }}</span>
          </div>
          <p v-if="r.closedAt" class="text-muted-foreground">
            {{ t("review.closedBy", { who: personLabel(r.closedBy, t("review.otherMember")), at: formatDateTime(r.closedAt, locale) }) }}
          </p>
          <p v-if="r.comment" class="whitespace-pre-wrap rounded bg-muted/50 p-2" data-testid="review-comment-text">{{ r.comment }}</p>
        </li>
      </ul>
    </section>
  </div>
</template>
