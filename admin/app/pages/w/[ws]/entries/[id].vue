<script setup lang="ts">
// 記事の編集画面（03-content-and-editing.md「下書き・公開・版」「同時編集」）。
// - 入力は自動で保存する（楽観ロック。Q73, Q137）
// - 公開は分割ボタン: 「公開」が主で、▼から「訂正として公開」（Q68）。どちらも確認画面を通す
// - 非公開と削除は、公開できる人（管理者・編集者とサーバー管理者）だけ
import { ChevronDown } from "@lucide/vue";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import EntryField from "~/components/entry/EntryField.vue";
import FreshnessPanel from "~/components/entry/FreshnessPanel.vue";
import PreviewPanel from "~/components/entry/PreviewPanel.vue";
import PublishDialog from "~/components/entry/PublishDialog.vue";
import EditLockBanner from "~/components/entry/EditLockBanner.vue";
import EntryTodoNotice from "~/components/entry/EntryTodoNotice.vue";
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

// タブ（内容のタブは、エディタの状態を保つため、ほかのタブを開いている間も残しておく）
// ?tab=review などで、開くタブを選べる（「要対応」の一覧から）
const TABS = ["content", "preview", "translations", "versions", "review", "freshness", "settings", "urls"];
const tab = ref(TABS.includes(String(route.query["tab"])) ? String(route.query["tab"]) : "content");
// プレビューのタブを一度でも開いたか（開いたら、ほかのタブに移っても iframe を残す）
const previewOpened = ref(tab.value === "preview");
watch(tab, (v) => v === "preview" && (previewOpened.value = true));
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

// 非公開・削除
const confirm = ref<"unpublish" | "delete" | null>(null);
const acting = ref(false);
const actionError = ref<unknown>(null);
async function runConfirmed() {
  acting.value = true;
  actionError.value = null;
  try {
    if (confirm.value === "unpublish") {
      await api.post(`${base}/unpublish`);
      lastPublished.value = null;
      published.value++;
      await refreshDraft();
    } else if (confirm.value === "delete") {
      await api.del(base);
      autosave.reset(0);
      await navigateTo(`/w/${ws}/entries`);
    }
    confirm.value = null;
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
  <div class="grid gap-6">
    <FormError :error="wsError ?? draftError" />
    <template v-if="draft && manifest && contentType">
      <div class="flex flex-wrap items-start gap-3">
        <div class="min-w-0 flex-1">
          <NuxtLink :to="`/w/${ws}/entries`" class="text-sm text-muted-foreground hover:text-foreground">{{ workspace?.name }}</NuxtLink>
          <h1 class="truncate text-2xl font-semibold">{{ title ?? t("entries.untitled") }}</h1>
          <div class="mt-1 flex flex-wrap items-center gap-2 text-sm text-muted-foreground">
            <Badge variant="outline">{{ draft.contentType }}</Badge>
            <Badge variant="outline">{{ draft.locale }}</Badge>
            <Badge :variant="draft.publishedVersionId ? 'default' : 'secondary'">
              {{ draft.publishedVersionId ? t("entries.published") : t("entries.draft") }}
            </Badge>
            <span role="status" aria-live="polite" data-testid="save-status">{{ statusText }}</span>
            <span v-if="hookFailure" class="text-destructive" data-testid="hook-failure">{{ t("entries.hookFailure") }}</span>
          </div>
        </div>
        <div v-if="canPublish(workspace)" class="flex items-center gap-2">
          <div class="flex">
            <Button class="rounded-r-none" :disabled="!editable" data-testid="publish" @click="openPublish(false)">{{ t("publish.button") }}</Button>
            <DropdownMenu>
              <DropdownMenuTrigger as-child>
                <Button class="rounded-l-none border-l border-l-primary-foreground/20 px-2" :disabled="!editable" :aria-label="t('publish.more')">
                  <ChevronDown class="size-4" />
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end">
                <DropdownMenuItem data-testid="publish-schedule" @select="openPublish(false, true)">{{ t("schedule.menu") }}</DropdownMenuItem>
                <DropdownMenuSeparator />
                <template v-if="hasPermission(workspace, 'corrections')">
                  <DropdownMenuItem data-testid="publish-correction" @select="openPublish(true)">{{ t("publish.asCorrection") }}</DropdownMenuItem>
                  <DropdownMenuSeparator />
                </template>
                <DropdownMenuItem :disabled="!draft.publishedVersionId" @select="confirm = 'unpublish'">{{ t("entries.unpublish") }}</DropdownMenuItem>
                <DropdownMenuItem class="text-destructive" @select="confirm = 'delete'">{{ t("entries.delete") }}</DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
          </div>
        </div>
        <!-- 訂正の権限だけを付与された人（執筆者など）は、訂正としての公開だけができる -->
        <Button v-else-if="hasPermission(workspace, 'corrections')" :disabled="!editable || !draft.publishedVersionId" data-testid="publish-correction" @click="openPublish(true)">
          {{ t("publish.asCorrection") }}
        </Button>
      </div>

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
      <EntryTodoNotice v-if="hasPermission(workspace, 'corrections')" :key="published" :workspace-id="ws" :entry-id="id" @open-tab="(name) => (tab = name)" />
      <p v-if="schema?.state === 'pending'" class="rounded-md border border-amber-500/50 bg-amber-500/10 p-3 text-sm" role="status">
        {{ t("entries.migrationPending") }}
      </p>
      <p v-if="lastPublished" class="rounded-md border p-3 text-sm" role="status">
        {{ lastPublished.isCorrection ? t("publish.doneCorrection", { url: lastPublished.url }) : t("publish.done", { url: lastPublished.url }) }}
      </p>

      <Tabs v-model="tab">
        <TabsList>
          <TabsTrigger value="content">{{ t("entryTabs.content") }}</TabsTrigger>
          <TabsTrigger v-if="canEdit(workspace)" value="preview" data-testid="tab-preview">{{ t("entryTabs.preview") }}</TabsTrigger>
          <TabsTrigger value="translations" data-testid="tab-translations">
            {{ t("entryTabs.translations") }}
            <span v-if="translation?.sourceUpdate" class="ml-1 inline-block size-2 rounded-full bg-amber-500" :aria-label="t('sourceUpdate.updated')" />
          </TabsTrigger>
          <TabsTrigger value="versions">{{ t("entryTabs.versions") }}</TabsTrigger>
          <TabsTrigger value="review" data-testid="tab-review">
            {{ t("entryTabs.review") }}
            <span v-if="review?.open" class="ml-1 inline-block size-2 rounded-full bg-amber-500" :aria-label="t('review.waiting')" />
          </TabsTrigger>
          <TabsTrigger value="freshness">{{ t("entryTabs.freshness") }}</TabsTrigger>
          <TabsTrigger value="settings">{{ t("entryTabs.settings") }}</TabsTrigger>
          <TabsTrigger value="urls">{{ t("entryTabs.urls") }}</TabsTrigger>
        </TabsList>
        <TabsContent value="content" force-mount class="mt-4 grid gap-6 data-[state=inactive]:hidden">
          <form class="grid gap-6" @submit.prevent>
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
          <p class="text-xs text-muted-foreground">{{ t("entries.updated", { at: formatDateTime(draft.updatedAt, locale) }) }}</p>
        </TabsContent>
        <!-- 一度開いたら、ほかのタブにいる間も残す（編集中も、保存のたびにサイトが取り直す） -->
        <TabsContent v-if="canEdit(workspace) && previewOpened" value="preview" force-mount class="mt-4 data-[state=inactive]:hidden">
          <PreviewPanel :workspace-id="ws" :entry-id="id" :workspace="workspace" :revision="autosave.revision.value" :flush="autosave.flush" />
        </TabsContent>
        <TabsContent value="translations" class="mt-4">
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
        <TabsContent value="versions" class="mt-4">
          <VersionsPanel :key="published" :workspace-id="ws" :entry-id="id" :content-type="contentType" :can-erase="canErase(workspace)" :can-write-note="hasPermission(workspace, 'corrections')" :custom-nodes="manifest?.body.customNodes ?? []" />
        </TabsContent>
        <TabsContent value="review" class="mt-4">
          <ReviewPanel :key="published" :workspace-id="ws" :entry-id="id" :can-edit="canEdit(workspace)" :can-publish="canPublish(workspace)" />
        </TabsContent>
        <TabsContent value="freshness" class="mt-4">
          <FreshnessPanel :key="published" :workspace-id="ws" :entry-id="id" :editable="canPublish(workspace)" :can-verify="hasPermission(workspace, 'corrections')" />
        </TabsContent>
        <TabsContent value="settings" class="mt-4">
          <SettingsPanel :workspace-id="ws" :entry-id="id" :editable="hasPermission(workspace, 'policySwitches')" />
        </TabsContent>
        <TabsContent value="urls" class="mt-4">
          <UrlHistoryPanel :key="published" :workspace-id="ws" :entry-id="id" />
        </TabsContent>
      </Tabs>

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

      <Dialog :open="confirm !== null" @update:open="(v) => !v && (confirm = null)">
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{{ confirm === "delete" ? t("entries.deleteTitle") : t("entries.unpublishTitle") }}</DialogTitle>
            <DialogDescription>{{ confirm === "delete" ? t("entries.deleteDescription") : t("entries.unpublishDescription") }}</DialogDescription>
          </DialogHeader>
          <FormError :error="actionError" />
          <DialogFooter>
            <Button variant="outline" @click="confirm = null">{{ t("common.cancel") }}</Button>
            <Button :variant="confirm === 'delete' ? 'destructive' : 'default'" :disabled="acting" @click="runConfirmed">
              {{ confirm === "delete" ? t("entries.delete") : t("entries.unpublish") }}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </template>
  </div>
</template>
