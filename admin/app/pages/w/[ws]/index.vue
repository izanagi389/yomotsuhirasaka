<script setup lang="ts">
// 「やること」（03-content-and-editing.md「やることの見せ方（Q67）」）。ワークスペースの入り口。
// その人が対応できる項目だけを、サーバーが優先度の順に並べて返す（server/src/http/admin_todo.rs、INV-TODO-01）。
// 項目を開くと、対応する記事のタブ（レビュー・版・鮮度・翻訳）を開く。一括の操作は作らない（Q46）。
import { Badge } from "@/components/ui/badge";
import NotificationCard from "~/components/admin/NotificationCard.vue";
import { type TodoItem, entryTitle, personLabel } from "~/utils/content";

const route = useRoute();
const ws = route.params.ws as string;
const { t, te, locale } = useI18n();
const { workspace, manifest, error } = await useWorkspace(ws);
const { data: items, error: todoError, refresh } = await useTodo(ws);

// 管理者への通知（計画の15番）: 「確認した」にすると一覧から消える。対象の人は、所属者の一覧から分かる人だけ名前で出す
const api = useApi();
const people = ref<Record<string, string>>({});
watch(
  () => (items.value ?? []).some((i) => i.kind === "notification"),
  async (has) => {
    if (!has || Object.keys(people.value).length > 0) return;
    try {
      const members = await api.get<{ userId: string; email: string }[]>(`/workspaces/${ws}/members`);
      people.value = Object.fromEntries(members.map((m) => [m.userId, m.email]));
    } catch {
      // 分からなければIDのまま
    }
  },
  { immediate: true },
);
const reading = ref(false);
const readError = ref<unknown>(null);
async function read(i: TodoItem) {
  if (!i.notification) return;
  reading.value = true;
  readError.value = null;
  try {
    await api.post(`/workspaces/${ws}/notifications/${i.notification.id}/read`);
    await refresh();
  } catch (e) {
    readError.value = e;
  } finally {
    reading.value = false;
  }
}

const titleOf = (i: TodoItem) => (i.entry ? entryTitle(manifest.value?.contentTypes.find((c) => c.name === i.entry!.contentType), i.entry.fields) : null);
const scheduleError = (code: string | null | undefined) => (code && te(`schedule.errors.${code}`) ? t(`schedule.errors.${code}`) : (code ?? ""));

/** 開くタブ（記事の画面の ?tab=）。 */
const TAB: Record<TodoItem["kind"], string | null> = {
  notification: null,
  budgetExhausted: null,
  embeddingModelMissing: null,
  scheduleFailed: null,
  scheduleRetrying: null,
  reviewWaiting: "review",
  unwrittenCorrection: "versions",
  reviewRejected: "review",
  sourceCorrected: "translations",
  reviewExpired: "freshness",
  sourceUpdated: "translations",
  scheduleCancelled: null,
};
const link = (i: TodoItem) =>
  i.kind === "embeddingModelMissing"
    ? "/server?tab=embedding"
    : i.kind === "budgetExhausted"
      ? `/w/${ws}/budget`
      : `/w/${ws}/entries/${i.entry?.id}${TAB[i.kind] ? `?tab=${TAB[i.kind]}` : ""}`;
const urgent = (k: TodoItem["kind"]) =>
  k === "budgetExhausted" || k === "embeddingModelMissing" || k === "scheduleFailed" || k === "scheduleRetrying" || k === "reviewExpired";

/** 種類ごとの補足（誰が・いつ・理由）。 */
function detail(i: TodoItem): string {
  const at = formatDateTime(i.at, locale.value);
  switch (i.kind) {
    case "notification":
    case "budgetExhausted":
      return at;
    case "embeddingModelMissing":
      return t("todo.embeddingModelMissingDetail");
    case "scheduleFailed":
    case "scheduleRetrying":
      return `${scheduleError(i.schedule?.lastError)} — ${t("todo.scheduledAt", { at: formatDateTime(i.schedule?.publishAt ?? i.at, locale.value) })}`;
    case "scheduleCancelled":
      return t("todo.scheduledAt", { at: formatDateTime(i.schedule?.publishAt ?? i.at, locale.value) });
    case "reviewWaiting":
      return t("review.requestedBy", { who: personLabel(i.request?.requestedBy, t("review.otherMember")), at });
    case "reviewRejected":
      return t("review.closedBy", { who: personLabel(i.request?.closedBy, t("review.otherMember")), at });
    case "unwrittenCorrection":
      return t("todo.correctedAt", { at });
    case "reviewExpired":
      return t("todo.dueAt", { at });
    case "sourceCorrected":
    case "sourceUpdated":
      return t("todo.sourcePublishedAt", { at });
  }
}
</script>

<template>
  <div class="grid gap-6">
    <PageHeader :title="t('nav.todo')" :description="workspace?.name" />
    <FormError :error="error ?? todoError ?? readError" />
    <p class="text-sm text-muted-foreground">{{ t("todo.help") }}</p>

    <p v-if="items && items.length === 0" class="text-sm text-muted-foreground" data-testid="todo-empty">{{ t("todo.none") }}</p>
    <ul v-else class="grid gap-2 text-sm" data-testid="todo-list">
      <li v-for="(i, n) in items ?? []" :key="`${i.kind}:${i.entry?.id ?? i.notification?.id}:${i.versionId ?? i.request?.id ?? i.schedule?.id ?? n}`">
        <NotificationCard v-if="i.kind === 'notification' && i.notification" :entry="i.notification" :people="people" :busy="reading" @read="read(i)" />
        <NuxtLink v-else :to="link(i)" class="grid gap-1 rounded-md border p-3 hover:bg-muted">
          <span class="flex flex-wrap items-center gap-2">
            <Badge :variant="urgent(i.kind) ? 'destructive' : 'outline'">{{ t(`todo.kind.${i.kind}`) }}</Badge>
            <Badge v-if="i.request?.assignedToMe && i.kind === 'reviewWaiting'" variant="secondary">{{ t("review.assignedToMe") }}</Badge>
            <span class="font-medium">{{ i.kind === "budgetExhausted" ? t(i.budget === "visitor_answers" ? "todo.answerBudgetExhausted" : i.budget === "editor_assist" ? "todo.assistBudgetExhausted" : "todo.budgetExhausted") : i.kind === "embeddingModelMissing" ? t("todo.embeddingModelMissing") : titleOf(i) || t("entries.untitled") }}</span>
            <span class="text-muted-foreground">{{ i.entry?.locale }}</span>
            <span class="ml-auto text-muted-foreground">{{ detail(i) }}</span>
          </span>
          <span v-if="i.kind === 'reviewRejected' && i.request?.comment" class="line-clamp-2 whitespace-pre-wrap text-muted-foreground">{{ i.request.comment }}</span>
        </NuxtLink>
      </li>
    </ul>
  </div>
</template>
