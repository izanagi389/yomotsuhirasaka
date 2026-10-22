<script setup lang="ts">
// 対応が必要な記事の一覧（06-corrections-and-freshness.md）: 説明を書いていない訂正と、確認期限が切れた公開中の記事。
// 一覧から記事を開いて、版のタブで説明を書き、鮮度のタブで「確認済み」を記録する（一括の操作は作らない。Q46）。
import type { ExpiredReview, UnwrittenCorrection } from "~/utils/content";

const route = useRoute();
const ws = route.params.ws as string;
const { t, locale } = useI18n();
const api = useApi();
const { workspace, error } = await useWorkspace(ws);
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
    <FormError :error="error ?? unwrittenError ?? expiredError" />

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
