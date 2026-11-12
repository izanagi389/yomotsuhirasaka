<script setup lang="ts">
// 記事の編集画面（03-content-and-editing.md「下書き・公開・版」「同時編集」）。
// - 入力は自動で保存する（楽観ロック。Q73, Q137）
// - 公開は分割ボタン: 「公開」が主で、▼から「訂正として公開」（Q68）。どちらも確認画面を通す
// - 非公開と削除は、公開できる人（管理者・編集者とサーバー管理者）だけ。画面の一番下の「危険な操作」に置き、確認してから行う
// - 画面の形: 上に帯（一覧へ戻る・状態・保存・プレビュー・公開）、中央に本文、右に欄（レビュー・履歴・翻訳・鮮度・関連・設定）
import { ArrowLeft, ChevronDown, Eye, Maximize2, Minimize2, PencilLine } from "@lucide/vue";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import EntryField from "~/components/entry/EntryField.vue";
import FreshnessPanel from "~/components/entry/FreshnessPanel.vue";
import PreviewPanel from "~/components/entry/PreviewPanel.vue";
import PublishDialog from "~/components/entry/PublishDialog.vue";
import EditLockBanner from "~/components/entry/EditLockBanner.vue";
import EntryTodoNotice from "~/components/entry/EntryTodoNotice.vue";
import RelatedPanel from "~/components/entry/RelatedPanel.vue";
import ReviewPanel from "~/components/entry/ReviewPanel.vue";
import ScheduleNotice from "~/components/entry/ScheduleNotice.vue";
import SourceDiffDialog from "~/components/entry/SourceDiffDialog.vue";
import SourceUpdateNotice from "~/components/entry/SourceUpdateNotice.vue";
import SettingsPanel from "~/components/entry/SettingsPanel.vue";
import TranslationsPanel from "~/components/entry/TranslationsPanel.vue";
import UrlHistoryPanel from "~/components/entry/UrlHistoryPanel.vue";
import VersionsPanel from "~/components/entry/VersionsPanel.vue";
import type { Draft, EntryReview, EntrySummary, FieldValue, Published, SavedDraft } from "~/utils/content";

const route = useRoute();
const ws = route.params.ws as string;
const id = route.params.id as string;
const { t, locale } = useI18n();
const api = useApi();
const base = `/workspaces/${ws}/entries/${id}`;

const { workspace, manifest, schema, error: wsError } = await useWorkspace(ws);
const { data: draft, error: draftError, refresh: refreshDraft } = await useAsyncData(`draft:${id}`, () => api.get<Draft>(base));
// 参照フィールドの候補
const { data: entries } = await useAsyncData(`entries:${ws}`, () => api.get<EntrySummary[]>(`/workspaces/${ws}/entries`));

const contentType = computed(() => manifest.value?.contentTypes.find((c) => c.name === draft.value?.contentType));
const values = reactive<Record<string, FieldValue>>({ ...(draft.value?.fields ?? {}) });
// 読み直したときにエディタを作り直すための番号
const generation = ref(0);

// 保存前のフックが失敗したとき（直さずに保存した）の種類。次の保存で消える
const hookFailure = ref<string | null>(null);
// 編集のロック（ソフトロック。Q73）。保存にはロックのIDを付ける
const lock = useEditLock(ws, id, () => canEdit(workspace.value));
const autosave = useAutosave(async (revision, fields) => {
  const lockId = lock.lockId.value;
  const saved = await api.put<SavedDraft>(`${base}/draft`, { revision, fields, ...(lockId ? { lockId } : {}) });
  hookFailure.value = saved.hookFailure ?? null;
  return saved;
}, draft.value?.revision ?? 0);
const editable = computed(
  () =>
    canEdit(workspace.value) &&
    schema.value?.state === "ok" &&
    lock.status.value === "mine" &&
    autosave.status.value !== "conflict" &&
    autosave.status.value !== "locked",
);
// 編集を引き継げるか（表示のため。照合はサーバーがワークスペースの設定と重い権限で行う）
const canTakeOver = computed(
  () => canEdit(workspace.value) && (workspace.value?.takeOverPolicy !== "permission" || hasPermission(workspace.value, "takeOverEditing")),
);
const takingOver = ref(false);
async function takeOver() {
  takingOver.value = true;
  try {
    if (!(await lock.takeOver())) return;
    // 保存できなかった入力があれば、それを保存し直す（リビジョンがずれていれば衝突として知らせる）。なければ最新を読み直す
    if (autosave.hasUnsaved.value) autosave.unlock();
    else await reload();
  } finally {
    takingOver.value = false;
  }
}

function change(name: string, value: FieldValue) {
  values[name] = value;
  autosave.set(name, value);
}

/** 下書きを読み直す（他の人の変更を取り込む。未保存の変更は捨てる）。 */
async function reload() {
  await refreshDraft();
  if (!draft.value) return;
  for (const k of Object.keys(values)) delete values[k];
  Object.assign(values, draft.value.fields);
  autosave.reset(draft.value.revision);
  generation.value++;
}

const statusText = computed(() => t(`entries.save.${autosave.status.value}`));
const title = computed(() => (draft.value ? entryTitle(contentType.value, values) : null));

// 中央は「本文」か「プレビュー」。本文のエディタは、プレビューを見ている間も状態を保つため残しておく
// 右の欄は、レビュー・履歴（版とURL）・翻訳・鮮度・関連・設定を切り替える
// ?tab=review などで、開く欄を選べる（「やること」の一覧から。前の形の名前も受け付ける）
const SIDE_TABS = ["review", "versions", "translations", "freshness", "related", "settings"];
const view = ref<"content" | "preview">("content");
const side = ref("review");
// 右の欄を広げているか（版の差分など、幅のいる内容を見るとき）
const sideWide = ref(false);
function openTab(name: string) {
  if (name === "content" || name === "preview") {
    view.value = name;
    return;
  }
  const s = name === "urls" ? "versions" : name;
  if (SIDE_TABS.includes(s)) side.value = s;
}
openTab(String(route.query["tab"] ?? "content"));
// プレビューを一度でも開いたか（開いたら、本文に戻っても iframe を残す）
const previewOpened = ref(view.value === "preview");
watch(view, (v) => v === "preview" && (previewOpened.value = true));
// レビュー待ちの印（レビューの欄と同じデータ）
const { data: review } = useAsyncData(`review:${id}`, () => api.get<EntryReview>(`${base}/review-request`));
// 翻訳の情報（原文の更新の知らせとタブの印。翻訳タブと共有する）
const { data: translation } = useTranslationInfo(ws, id);
const sourceDiffOpen = ref(false);
// 公開・非公開の後に、版とURLの履歴を読み直すための番号
const published = ref(0);

// 公開
const publishOpen = ref(false);
const asCorrection = ref(false);
const asSchedule = ref(false);
const lastPublished = ref<Published | null>(null);
function openPublish(correction: boolean, schedule = false) {
  asCorrection.value = correction;
  asSchedule.value = schedule;
  publishOpen.value = true;
}
async function onPublished(result: Published) {
  lastPublished.value = result;
  published.value++;
  await refreshDraft();
  // その場の公開と訂正は、予約を取り消す
  await refreshNuxtData([`schedule:${id}`, `review:${id}`]);
}
async function onScheduled() {
  // 予約すると、開いているレビュー依頼は「予約済み」で閉じる
  await refreshNuxtData([`schedule:${id}`, `review:${id}`]);
}

// 非公開・削除（確認してから行う）
const { confirm } = useConfirm();
const acting = ref(false);
const actionError = ref<unknown>(null);
async function unpublish() {
  const ok = await confirm({
    title: t("entries.unpublishTitle"),
    description: t("entries.unpublishDescription"),
    confirmLabel: t("entries.unpublish"),
    destructive: true,
  });
  if (!ok) return;
  await act(async () => {
    await api.post(`${base}/unpublish`);
    lastPublished.value = null;
    published.value++;
    await refreshDraft();
  });
}
async function remove() {
  const ok = await confirm({
    title: t("entries.deleteTitle"),
    description: t("entries.deleteDescription"),
    confirmLabel: t("entries.delete"),
    destructive: true,
  });
  if (!ok) return;
  await act(async () => {
    await api.del(base);
    autosave.reset(0);
    await navigateTo(`/w/${ws}/entries`);
  });
}
async function act(run: () => Promise<void>) {
  acting.value = true;
  actionError.value = null;
  try {
    await run();
  } catch (e) {
    actionError.value = e;
  } finally {
    acting.value = false;
  }
}

onBeforeRouteLeave(async () => {
  if (autosave.hasUnsaved.value && autosave.status.value !== "conflict") await autosave.flush();
  if (autosave.hasUnsaved.value && !window.confirm(t("entries.leaveUnsaved"))) return false;
});
</script>

<template>
  <div class="flex min-h-svh flex-col">
    <FormError :error="wsError ?? draftError" class="m-4" />
    <template v-if="draft && manifest && contentType">
      <!-- 上の帯: 一覧へ戻る・題名・状態・保存の状態・プレビュー・公開 -->
      <header class="sticky top-0 z-20 flex flex-wrap items-center gap-x-3 gap-y-2 border-b bg-card px-4 py-2.5 md:px-6">
        <NuxtLink :to="`/w/${ws}/entries`" class="flex items-center gap-1.5 text-sm text-muted-foreground hover:text-foreground">
          <ArrowLeft class="size-4" aria-hidden="true" />{{ t("entries.backToList") }}
        </NuxtLink>
        <span aria-hidden="true" class="text-border">／</span>
        <h1 class="min-w-0 max-w-[40ch] truncate text-sm font-medium">{{ title ?? t("entries.untitled") }}</h1>
        <span class="inline-flex items-center gap-1.5 text-xs font-medium" :class="draft.publishedVersionId ? 'text-emerald-700 dark:text-emerald-400' : 'text-muted-foreground'">
          <svg aria-hidden="true" width="10" height="10" viewBox="0 0 12 12"><path d="M6 0.8L11.2 6L6 11.2L0.8 6Z" fill="currentColor" /></svg>
          {{ draft.publishedVersionId ? t("entries.published") : t("entries.draft") }}
        </span>
        <Badge variant="outline" class="font-normal">{{ draft.contentType }}</Badge>
        <Badge variant="outline" class="font-normal">{{ draft.locale }}</Badge>
        <span v-if="hookFailure" class="text-xs text-destructive" data-testid="hook-failure">{{ t("entries.hookFailure") }}</span>

        <div class="ml-auto flex flex-wrap items-center gap-2">
          <span class="text-xs text-muted-foreground" role="status" aria-live="polite" data-testid="save-status">{{ statusText }}</span>
          <Button
            v-if="canEdit(workspace)"
            size="sm"
            variant="outline"
            :aria-pressed="view === 'preview'"
            data-testid="tab-preview"
            @click="view = view === 'preview' ? 'content' : 'preview'"
          >
            <PencilLine v-if="view === 'preview'" aria-hidden="true" />
            <Eye v-else aria-hidden="true" />
            {{ view === "preview" ? t("entries.backToEdit") : t("entryTabs.preview") }}
          </Button>
          <div v-if="canPublish(workspace)" class="flex">
            <Button size="sm" class="rounded-r-none" :disabled="!editable" data-testid="publish" @click="openPublish(false)">{{ t("publish.button") }}</Button>
            <DropdownMenu>
              <DropdownMenuTrigger as-child>
                <Button size="sm" class="rounded-l-none border-l border-l-primary-foreground/20 px-2" :disabled="!editable" :aria-label="t('publish.more')">
                  <ChevronDown class="size-4" />
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end">
                <DropdownMenuItem data-testid="publish-schedule" @select="openPublish(false, true)">{{ t("schedule.menu") }}</DropdownMenuItem>
                <template v-if="hasPermission(workspace, 'corrections')">
                  <DropdownMenuSeparator />
                  <DropdownMenuItem data-testid="publish-correction" @select="openPublish(true)">{{ t("publish.asCorrection") }}</DropdownMenuItem>
                </template>
              </DropdownMenuContent>
            </DropdownMenu>
          </div>
          <!-- 訂正の権限だけを付与された人（執筆者など）は、訂正としての公開だけができる -->
          <Button
            v-else-if="hasPermission(workspace, 'corrections')"
            size="sm"
            :disabled="!editable || !draft.publishedVersionId"
            data-testid="publish-correction"
            @click="openPublish(true)"
          >
            {{ t("publish.asCorrection") }}
          </Button>
        </div>
      </header>

      <div class="flex flex-1 flex-col lg:flex-row">
        <!-- 中央: お知らせと本文（またはプレビュー） -->
        <div class="min-w-0 flex-1 px-4 py-6 md:px-8" :class="sideWide ? 'max-lg:block lg:hidden' : ''">
          <div class="mx-auto grid max-w-3xl gap-4" :class="view === 'preview' ? 'max-w-none' : ''">
            <div v-if="autosave.status.value === 'conflict'" class="flex flex-wrap items-center gap-3 rounded-md border border-destructive/50 bg-destructive/10 p-3 text-sm" role="alert">
              <span class="flex-1">{{ t("entries.conflict") }}</span>
              <Button size="sm" variant="outline" @click="reload">{{ t("entries.reload") }}</Button>
            </div>
            <FormError v-else-if="autosave.status.value === 'error'" :error="autosave.error.value" />
            <EditLockBanner
              :status="lock.status.value"
              :holder="lock.holder.value"
              :since="lock.since.value"
              :can-take-over="canTakeOver"
              :unsaved="autosave.hasUnsaved.value"
              :busy="takingOver"
              @take-over="takeOver"
            />
            <FormError :error="lock.error.value" />
            <ScheduleNotice :key="published" :workspace-id="ws" :entry-id="id" :can-publish="canPublish(workspace)" @reschedule="openPublish(false, true)" />
            <SourceUpdateNotice :workspace-id="ws" :entry-id="id" @show-diff="sourceDiffOpen = true" />
            <EntryTodoNotice v-if="hasPermission(workspace, 'corrections')" :key="published" :workspace-id="ws" :entry-id="id" @open-tab="openTab" />
            <p v-if="schema?.state === 'pending'" class="rounded-md border border-amber-500/50 bg-amber-500/10 p-3 text-sm" role="status">
              {{ t("entries.migrationPending") }}
            </p>
            <p v-if="lastPublished" class="rounded-md border p-3 text-sm" role="status">
              {{ lastPublished.isCorrection ? t("publish.doneCorrection", { url: lastPublished.url }) : t("publish.done", { url: lastPublished.url }) }}
            </p>

            <form v-show="view === 'content'" class="grid gap-6" @submit.prevent>
              <EntryField
                v-for="f in contentType.fields"
                :key="`${f.name}:${generation}`"
                :field="f"
                :model-value="values[f.name]"
                :manifest="manifest"
                :workspace-id="ws"
                :locale="draft.locale"
                :editable="editable"
                :entries="entries ?? []"
                @update:model-value="change(f.name, $event)"
              />
            </form>
            <!-- 一度開いたら、本文に戻っても残す（編集中も、保存のたびにサイトが取り直す） -->
            <div v-if="canEdit(workspace) && previewOpened" v-show="view === 'preview'">
              <PreviewPanel :workspace-id="ws" :entry-id="id" :workspace="workspace" :revision="autosave.revision.value" :flush="autosave.flush" />
            </div>
            <p v-show="view === 'content'" class="text-xs text-muted-foreground">{{ t("entries.updated", { at: formatDateTime(draft.updatedAt, locale) }) }}</p>

            <!-- 危険な操作（公開できる人だけ） -->
            <details v-if="canPublish(workspace)" v-show="view === 'content'" class="mt-6 rounded-lg border border-destructive/30 p-4 text-sm" data-testid="entry-danger">
              <summary class="cursor-pointer font-medium text-destructive">{{ t("dangerZone.title") }}</summary>
              <div class="mt-3 flex flex-wrap gap-2">
                <Button variant="outline" size="sm" class="text-destructive hover:text-destructive" :disabled="acting || !draft.publishedVersionId" data-testid="entry-unpublish" @click="unpublish">
                  {{ t("entries.unpublish") }}
                </Button>
                <Button variant="destructive" size="sm" :disabled="acting" data-testid="entry-delete" @click="remove">{{ t("entries.delete") }}</Button>
              </div>
              <FormError :error="actionError" class="mt-3" />
            </details>
          </div>
        </div>

        <!-- 右の欄 -->
        <aside
          class="shrink-0 border-t bg-card lg:sticky lg:top-[53px] lg:h-[calc(100svh-53px)] lg:overflow-y-auto lg:border-t-0 lg:border-l"
          :class="sideWide ? 'lg:flex-1' : 'lg:w-[400px]'"
          :aria-label="t('entries.sidePanel')"
        >
          <Tabs v-model="side">
            <div class="sticky top-0 z-10 flex items-start gap-1 border-b bg-card px-3 pt-3 pb-2">
              <TabsList class="flex-1">
                <TabsTrigger value="review" data-testid="tab-review">
                  {{ t("entryTabs.review") }}
                  <span v-if="review?.open" class="inline-block size-2 rounded-full bg-amber-500" :aria-label="t('review.waiting')" />
                </TabsTrigger>
                <TabsTrigger value="versions">{{ t("entryTabs.history") }}</TabsTrigger>
                <TabsTrigger value="translations" data-testid="tab-translations">
                  {{ t("entryTabs.translations") }}
                  <span v-if="translation?.sourceUpdate" class="inline-block size-2 rounded-full bg-amber-500" :aria-label="t('sourceUpdate.updated')" />
                </TabsTrigger>
                <TabsTrigger value="freshness">{{ t("entryTabs.freshness") }}</TabsTrigger>
                <TabsTrigger value="related" data-testid="tab-related">{{ t("entryTabs.related") }}</TabsTrigger>
                <TabsTrigger value="settings">{{ t("entryTabs.settings") }}</TabsTrigger>
              </TabsList>
              <Button
                variant="ghost"
                size="icon-sm"
                class="max-lg:hidden"
                :aria-label="sideWide ? t('entries.sideNarrow') : t('entries.sideWide')"
                :aria-pressed="sideWide"
                @click="sideWide = !sideWide"
              >
                <Minimize2 v-if="sideWide" aria-hidden="true" />
                <Maximize2 v-else aria-hidden="true" />
              </Button>
            </div>
            <div class="p-4 text-sm">
              <TabsContent value="review">
                <ReviewPanel :key="published" :workspace-id="ws" :entry-id="id" :can-edit="canEdit(workspace)" :can-publish="canPublish(workspace)" />
              </TabsContent>
              <TabsContent value="versions" class="grid gap-8">
                <VersionsPanel :key="published" :workspace-id="ws" :entry-id="id" :content-type="contentType" :can-erase="canErase(workspace)" :can-write-note="hasPermission(workspace, 'corrections')" :custom-nodes="manifest?.body.customNodes ?? []" />
                <section class="grid gap-2">
                  <h2 class="text-base font-semibold">{{ t("entryTabs.urls") }}</h2>
                  <UrlHistoryPanel :key="published" :workspace-id="ws" :entry-id="id" />
                </section>
              </TabsContent>
              <TabsContent value="translations">
                <TranslationsPanel
                  :workspace-id="ws"
                  :entry-id="id"
                  :locales="manifest.locales"
                  :can-create="editable"
                  :can-set-source="canPublish(workspace)"
                  :flush="autosave.flush"
                  @show-diff="sourceDiffOpen = true"
                />
              </TabsContent>
              <TabsContent value="freshness">
                <FreshnessPanel :key="published" :workspace-id="ws" :entry-id="id" :editable="canPublish(workspace)" :can-verify="hasPermission(workspace, 'corrections')" />
              </TabsContent>
              <TabsContent value="related">
                <RelatedPanel :workspace-id="ws" :entry-id="id" :content-type="contentType" :locale="draft.locale" :entries="entries ?? []" :editable="canPublish(workspace)" />
              </TabsContent>
              <TabsContent value="settings">
                <SettingsPanel :workspace-id="ws" :entry-id="id" :editable="hasPermission(workspace, 'policySwitches')" />
              </TabsContent>
            </div>
          </Tabs>
        </aside>
      </div>

      <PublishDialog
        v-model:open="publishOpen"
        :workspace-id="ws"
        :entry-id="id"
        :locale="draft.locale"
        :correction="asCorrection"
        :schedule="asSchedule"
        :flush="autosave.flush"
        :can-reflect="canEdit(workspace)"
        @published="onPublished"
        @scheduled="onScheduled"
      />

      <SourceDiffDialog v-model:open="sourceDiffOpen" :workspace-id="ws" :entry-id="id" :can-reflect="canEdit(workspace)" />
    </template>
  </div>
</template>
