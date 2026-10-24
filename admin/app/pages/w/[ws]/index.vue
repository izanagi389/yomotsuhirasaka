<script setup lang="ts">
// 「やること」（03-content-and-editing.md「やることの見せ方（Q67）」）。ワークスペースの入り口。
// その人が対応できる項目だけを、サーバーが優先度の順に並べて返す（server/src/http/admin_todo.rs、INV-TODO-01）。
// 項目を開くと、対応する記事のタブ（レビュー・版・鮮度・翻訳）を開く。一括の操作は作らない（Q46）。
import { Badge } from "@/components/ui/badge";
import { type TodoItem, entryTitle, personLabel } from "~/utils/content";

const route = useRoute();
const ws = route.params.ws as string;
const { t, te, locale } = useI18n();
const { workspace, manifest, error } = await useWorkspace(ws);
const { data: items, error: todoError } = await useTodo(ws);

const titleOf = (i: TodoItem) => entryTitle(manifest.value?.contentTypes.find((c) => c.name === i.entry.contentType), i.entry.fields);
const scheduleError = (code: string | null | undefined) => (code && te(`schedule.errors.${code}`) ? t(`schedule.errors.${code}`) : (code ?? ""));

/** 開くタブ（記事の画面の ?tab=）。 */
const TAB: Record<TodoItem["kind"], string | null> = {
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
const link = (i: TodoItem) => `/w/${ws}/entries/${i.entry.id}${TAB[i.kind] ? `?tab=${TAB[i.kind]}` : ""}`;
const urgent = (k: TodoItem["kind"]) => k === "scheduleFailed" || k === "scheduleRetrying" || k === "reviewExpired";

/** 種類ごとの補足（誰が・いつ・理由）。 */
function detail(i: TodoItem): string {
  const at = formatDateTime(i.at, locale.value);
  switch (i.kind) {
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
    <WorkspaceNav :workspace-id="ws" :name="workspace?.name" :admin="isWorkspaceAdmin(workspace)" />
    <FormError :error="error ?? todoError" />
    <p class="text-sm text-muted-foreground">{{ t("todo.help") }}</p>

    <p v-if="items && items.length === 0" class="text-sm text-muted-foreground" data-testid="todo-empty">{{ t("todo.none") }}</p>
    <ul v-else class="grid gap-2 text-sm" data-testid="todo-list">
      <li v-for="(i, n) in items ?? []" :key="`${i.kind}:${i.entry.id}:${i.versionId ?? i.request?.id ?? i.schedule?.id ?? n}`">
        <NuxtLink :to="link(i)" class="grid gap-1 rounded-md border p-3 hover:bg-muted">
          <span class="flex flex-wrap items-center gap-2">
            <Badge :variant="urgent(i.kind) ? 'destructive' : 'outline'">{{ t(`todo.kind.${i.kind}`) }}</Badge>
            <Badge v-if="i.request?.assignedToMe && i.kind === 'reviewWaiting'" variant="secondary">{{ t("review.assignedToMe") }}</Badge>
            <span class="font-medium">{{ titleOf(i) || t("entries.untitled") }}</span>
            <span class="text-muted-foreground">{{ i.entry.locale }}</span>
            <span class="ml-auto text-muted-foreground">{{ detail(i) }}</span>
          </span>
          <span v-if="i.kind === 'reviewRejected' && i.request?.comment" class="line-clamp-2 whitespace-pre-wrap text-muted-foreground">{{ i.request.comment }}</span>
        </NuxtLink>
      </li>
    </ul>
  </div>
</template>
