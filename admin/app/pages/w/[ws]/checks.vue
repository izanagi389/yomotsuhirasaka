<script setup lang="ts">
// 対応が必要な記事の一覧（06-corrections-and-freshness.md）: 説明を書いていない訂正と、確認期限が切れた公開中の記事。
// 一覧から記事を開いて、版のタブで説明を書き、鮮度のタブで「確認済み」を記録する（一括の操作は作らない。Q46）。
// レビュー依頼（計画の8番）: 公開できる人には「レビュー待ち」（自分が指定されたものが先）、依頼した人には「差し戻された記事」。
// 予約公開（計画の9番）: 公開できる人には「公開を待っている予約」（やり直し中）、予約した人には「取り消し・失敗した予約」。
import { Badge } from "@/components/ui/badge";
import { type ExpiredReview, type ReviewLists, type ScheduleLists, type UnwrittenCorrection, entryTitle, personLabel } from "~/utils/content";

const route = useRoute();
const ws = route.params.ws as string;
const { t, te, locale } = useI18n();
const api = useApi();
const { workspace, manifest, error } = await useWorkspace(ws);
const { data: reviews, error: reviewsError } = await useAsyncData(`review-requests:${ws}`, () =>
  api.get<ReviewLists>(`/workspaces/${ws}/review-requests`),
);
const { data: schedules, error: schedulesError } = await useAsyncData(`schedules:${ws}`, () => api.get<ScheduleLists>(`/workspaces/${ws}/schedules`));
const scheduleError = (code: string | null) => (code && te(`schedule.errors.${code}`) ? t(`schedule.errors.${code}`) : (code ?? ""));
const titleOf = (e: ReviewLists["waiting"][number]["entry"]) =>
  entryTitle(manifest.value?.contentTypes.find((c) => c.name === e.contentType), e.fields);
const { data: unwritten, error: unwrittenError } = await useAsyncData(`unwritten:${ws}`, () =>
  api.get<UnwrittenCorrection[]>(`/workspaces/${ws}/corrections/unwritten`),
);
const { data: expired, error: expiredError } = await useAsyncData(`expired:${ws}`, () =>
  api.get<ExpiredReview[]>(`/workspaces/${ws}/reviews/expired`),
);
</script>

<template>
  <div class="grid gap-6">
    <WorkspaceNav :workspace-id="ws" :name="workspace?.name" :admin="isWorkspaceAdmin(workspace)" />
    <FormError :error="error ?? reviewsError ?? schedulesError ?? unwrittenError ?? expiredError" />

    <section v-if="schedules && schedules.retrying.length > 0" class="grid gap-2 text-sm" data-testid="schedule-retrying-list">
      <h2 class="text-lg font-medium">{{ t("checks.scheduleRetrying") }}</h2>
      <p class="text-muted-foreground">{{ t("checks.scheduleRetryingHelp") }}</p>
      <ul class="grid gap-2">
        <li v-for="i in schedules.retrying" :key="i.schedule.id">
          <NuxtLink :to="`/w/${ws}/entries/${i.entry.id}`" class="flex flex-wrap items-center gap-2 rounded-md border p-3 hover:bg-muted">
            <span class="font-medium">{{ titleOf(i.entry) || t("entries.untitled") }}</span>
            <span class="text-muted-foreground">{{ i.entry.locale }}</span>
            <span class="text-destructive">{{ scheduleError(i.schedule.lastError) }}</span>
            <span class="ml-auto text-muted-foreground">{{ t("checks.scheduledAt", { at: formatDateTime(i.schedule.publishAt, locale) }) }}</span>
          </NuxtLink>
        </li>
      </ul>
    </section>

    <section v-if="schedules && schedules.attention.length > 0" class="grid gap-2 text-sm" data-testid="schedule-attention-list">
      <h2 class="text-lg font-medium">{{ t("checks.scheduleAttention") }}</h2>
      <p class="text-muted-foreground">{{ t("checks.scheduleAttentionHelp") }}</p>
      <ul class="grid gap-2">
        <li v-for="i in schedules.attention" :key="i.schedule.id">
          <NuxtLink :to="`/w/${ws}/entries/${i.entry.id}`" class="flex flex-wrap items-center gap-2 rounded-md border p-3 hover:bg-muted">
            <span class="font-medium">{{ titleOf(i.entry) || t("entries.untitled") }}</span>
            <span class="text-muted-foreground">{{ i.entry.locale }}</span>
            <Badge :variant="i.schedule.state === 'failed' ? 'destructive' : 'outline'">
              {{ i.schedule.state === "failed" ? t("checks.scheduleFailed") : t("checks.scheduleCorrected") }}
            </Badge>
            <span class="ml-auto text-muted-foreground">{{ t("checks.scheduledAt", { at: formatDateTime(i.schedule.publishAt, locale) }) }}</span>
          </NuxtLink>
        </li>
      </ul>
    </section>

    <section v-if="canPublish(workspace)" class="grid gap-2 text-sm" data-testid="review-waiting">
      <h2 class="text-lg font-medium">{{ t("checks.reviewWaiting") }}</h2>
      <p class="text-muted-foreground">{{ t("checks.reviewWaitingHelp") }}</p>
      <p v-if="reviews && reviews.waiting.length === 0" class="text-muted-foreground">{{ t("checks.none") }}</p>
      <ul v-else class="grid gap-2">
        <li v-for="i in reviews?.waiting ?? []" :key="i.request.id">
          <NuxtLink :to="`/w/${ws}/entries/${i.entry.id}?tab=review`" class="flex flex-wrap items-center gap-2 rounded-md border p-3 hover:bg-muted">
            <span class="font-medium">{{ titleOf(i.entry) || t("entries.untitled") }}</span>
            <span class="text-muted-foreground">{{ i.entry.locale }}</span>
            <Badge v-if="i.request.assignedToMe" variant="secondary">{{ t("review.assignedToMe") }}</Badge>
            <Badge v-if="i.request.changedSinceRequest" variant="outline">{{ t("review.changedSinceRequest") }}</Badge>
            <span class="ml-auto text-muted-foreground">
              {{ t("review.requestedBy", { who: personLabel(i.request.requestedBy, t("review.otherMember")), at: formatDateTime(i.request.createdAt, locale) }) }}
            </span>
          </NuxtLink>
        </li>
      </ul>
    </section>

    <section v-if="reviews && reviews.rejected.length > 0" class="grid gap-2 text-sm" data-testid="review-rejected">
      <h2 class="text-lg font-medium">{{ t("checks.reviewRejected") }}</h2>
      <p class="text-muted-foreground">{{ t("checks.reviewRejectedHelp") }}</p>
      <ul class="grid gap-2">
        <li v-for="i in reviews.rejected" :key="i.request.id">
          <NuxtLink :to="`/w/${ws}/entries/${i.entry.id}?tab=review`" class="grid gap-1 rounded-md border p-3 hover:bg-muted">
            <span class="flex flex-wrap items-center gap-2">
              <span class="font-medium">{{ titleOf(i.entry) || t("entries.untitled") }}</span>
              <span class="text-muted-foreground">{{ i.entry.locale }}</span>
              <span class="ml-auto text-muted-foreground">
                {{ t("review.closedBy", { who: personLabel(i.request.closedBy, t("review.otherMember")), at: formatDateTime(i.request.closedAt ?? "", locale) }) }}
              </span>
            </span>
            <span class="line-clamp-2 whitespace-pre-wrap text-muted-foreground">{{ i.request.comment }}</span>
          </NuxtLink>
        </li>
      </ul>
    </section>

    <section class="grid gap-2 text-sm" data-testid="unwritten-corrections">
      <h2 class="text-lg font-medium">{{ t("checks.unwritten") }}</h2>
      <p class="text-muted-foreground">{{ t("checks.unwrittenHelp") }}</p>
      <p v-if="unwritten && unwritten.length === 0" class="text-muted-foreground">{{ t("checks.none") }}</p>
      <ul v-else class="grid gap-2">
        <li v-for="u in unwritten ?? []" :key="u.versionId">
          <NuxtLink :to="`/w/${ws}/entries/${u.entryId}`" class="flex flex-wrap items-center gap-2 rounded-md border p-3 hover:bg-muted">
            <span class="font-medium">{{ u.title || t("entries.untitled") }}</span>
            <span class="text-muted-foreground">{{ u.locale }}</span>
            <span class="ml-auto text-muted-foreground">{{ t("checks.correctedAt", { at: formatDateTime(u.correctedAt, locale) }) }}</span>
          </NuxtLink>
        </li>
      </ul>
    </section>

    <section class="grid gap-2 text-sm" data-testid="expired-reviews">
      <h2 class="text-lg font-medium">{{ t("checks.expired") }}</h2>
      <p class="text-muted-foreground">{{ t("checks.expiredHelp") }}</p>
      <p v-if="expired && expired.length === 0" class="text-muted-foreground">{{ t("checks.none") }}</p>
      <ul v-else class="grid gap-2">
        <li v-for="e in expired ?? []" :key="e.entryId">
          <NuxtLink :to="`/w/${ws}/entries/${e.entryId}`" class="flex flex-wrap items-center gap-2 rounded-md border p-3 hover:bg-muted">
            <span class="font-medium">{{ e.title || t("entries.untitled") }}</span>
            <span class="text-muted-foreground">{{ e.locale }}</span>
            <span class="ml-auto text-destructive">{{ t("checks.dueAt", { at: formatDateTime(e.reviewDueAt, locale) }) }}</span>
          </NuxtLink>
        </li>
      </ul>
    </section>
  </div>
</template>
