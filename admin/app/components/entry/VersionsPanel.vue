<script setup lang="ts">
// 版の一覧と中身、版の本文の抹消（03-content-and-editing.md「版の本文の抹消」、Q134, Q144, Q152, Q158）。
// - 抹消は管理者とサーバー管理者だけ。再認証を求める（サーバーが reauth_required を返し、ダイアログで確認してから送り直す）
// - 現在公開中の版は抹消できない。直した版を（訂正として）公開するか、非公開にしてからにするよう案内する
// - どの記事でも使われていないメディアを一覧にし、既定で選んだ状態で一緒に削除できる
// - 同じ翻訳グループの他のロケールの記事に、同じ内容が残っているかもしれないことを知らせる
// - 抹消した内容は、それ以前のDBのバックアップには残ることを表示する
// - 訂正として公開した版の説明を、後から書き直せる（公開できる人だけ）。編集の履歴は管理画面だけに出す（Q63、INV-COR-04）
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Textarea } from "@/components/ui/textarea";
import ReadOnlyBody from "~/components/editor/ReadOnlyBody.vue";
import { mediaInfo } from "~/components/editor/media";
import type { CustomNodeDecl } from "@yomotsuhirasaka/editor-schema";
import type { ContentType, EraseCandidates, MediaInfo, NoteEdit, VersionDetail, VersionInfo } from "~/utils/content";

const props = defineProps<{
  workspaceId: string;
  entryId: string;
  contentType: ContentType;
  canErase: boolean;
  /** 訂正の説明を書き直せるか。 */
  canWriteNote: boolean;
  /** カスタムノードの宣言（版の本文の表示に使う）。 */
  customNodes?: CustomNodeDecl[];
}>();
const { t, locale } = useI18n();
const api = useApi();
const base = computed(() => `/workspaces/${props.workspaceId}/entries/${props.entryId}/versions`);

const { data: versions, error, refresh } = useAsyncData(`versions:${props.entryId}`, () => api.get<VersionInfo[]>(base.value));
const newestFirst = computed(() => [...(versions.value ?? [])].reverse());

// 中身を見る
const viewing = ref<VersionDetail | null>(null);
const viewError = ref<unknown>(null);
async function view(v: VersionInfo) {
  viewError.value = null;
  try {
    viewing.value = await api.get<VersionDetail>(`${base.value}/${v.id}`);
  } catch (e) {
    viewError.value = e;
  }
}

// 抹消
const erasing = ref<VersionInfo | null>(null);
const candidates = ref<EraseCandidates | null>(null);
const media = ref<Record<string, MediaInfo | null>>({});
const selected = ref<string[]>([]);
const eraseError = ref<unknown>(null);
const busy = ref(false);
async function openErase(v: VersionInfo) {
  erasing.value = v;
  candidates.value = null;
  eraseError.value = null;
  try {
    const c = await api.get<EraseCandidates>(`${base.value}/${v.id}/erase-candidates`);
    candidates.value = c;
    // 既定で選んだ状態にする（Q158）
    selected.value = [...c.media];
    for (const id of c.media) void mediaInfo(props.workspaceId, id).then((m) => (media.value = { ...media.value, [id]: m }));
  } catch (e) {
    eraseError.value = e;
  }
}
function toggle(id: string, on: boolean) {
  selected.value = on ? [...selected.value, id] : selected.value.filter((x) => x !== id);
}
async function erase() {
  if (!erasing.value) return;
  busy.value = true;
  eraseError.value = null;
  try {
    await api.post(`${base.value}/${erasing.value.id}/erase`, { deleteMediaIds: selected.value });
    if (viewing.value?.id === erasing.value.id) viewing.value = null;
    erasing.value = null;
    await refresh();
  } catch (e) {
    eraseError.value = e;
  } finally {
    busy.value = false;
  }
}

// 訂正の説明
const noting = ref<VersionInfo | null>(null);
const noteText = ref("");
const noteHistory = ref<NoteEdit[]>([]);
const noteError = ref<unknown>(null);
async function openNote(v: VersionInfo) {
  noting.value = v;
  noteText.value = v.correctionNote ?? "";
  noteHistory.value = [];
  noteError.value = null;
  try {
    noteHistory.value = await api.get<NoteEdit[]>(`${base.value}/${v.id}/correction-note/history`);
  } catch (e) {
    noteError.value = e;
  }
}
async function saveNote() {
  if (!noting.value) return;
  busy.value = true;
  noteError.value = null;
  try {
    await api.put(`${base.value}/${noting.value.id}/correction-note`, { text: noteText.value });
    noting.value = null;
    await refresh();
  } catch (e) {
    noteError.value = e;
  } finally {
    busy.value = false;
  }
}

const textFields = computed(() => props.contentType.fields.filter((f) => f.type === "text" || f.type === "slug"));
const richFields = computed(() => props.contentType.fields.filter((f) => f.type === "richtext"));
</script>

<template>
  <div class="grid gap-4 text-sm">
    <FormError :error="error ?? viewError" />
    <p v-if="versions && versions.length === 0" class="text-muted-foreground">{{ t("versions.empty") }}</p>
    <ul class="grid gap-2">
      <li v-for="v in newestFirst" :key="v.id" class="flex flex-wrap items-center gap-2 rounded-md border p-3">
        <span class="font-medium">{{ t("versions.number", { n: v.number }) }}</span>
        <span class="text-muted-foreground">{{ formatDateTime(v.publishedAt, locale) }}</span>
        <Badge v-if="v.current">{{ t("versions.current") }}</Badge>
        <Badge v-if="v.isCorrection" variant="outline">{{ t("versions.correction") }}</Badge>
        <Badge v-if="v.erased" variant="destructive">{{ t("versions.erased") }}</Badge>
        <Badge v-if="v.isCorrection && !v.correctionNote" variant="secondary" data-testid="note-unwritten">{{ t("corrections.unwritten") }}</Badge>
        <div class="ml-auto flex gap-2">
          <Button v-if="v.isCorrection && canWriteNote" size="sm" variant="outline" @click="openNote(v)">{{ t("corrections.editNote") }}</Button>
          <Button v-if="!v.erased" size="sm" variant="outline" @click="view(v)">{{ t("versions.view") }}</Button>
          <Button v-if="canErase && !v.erased" size="sm" variant="outline" class="text-destructive" @click="openErase(v)">{{ t("versions.erase") }}</Button>
        </div>
        <p v-if="v.isCorrection && v.correctionNote" class="w-full whitespace-pre-line text-muted-foreground">{{ v.correctionNote }}</p>
      </li>
    </ul>

    <section v-if="viewing && viewing.fields" class="grid gap-4 rounded-md border p-4" :aria-label="t('versions.number', { n: viewing.number })">
      <div class="flex items-center gap-2">
        <h3 class="font-medium">{{ t("versions.number", { n: viewing.number }) }}</h3>
        <span class="text-muted-foreground">{{ viewing.url }}</span>
        <Button size="sm" variant="ghost" class="ml-auto" @click="viewing = null">{{ t("versions.close") }}</Button>
      </div>
      <dl class="grid gap-2">
        <template v-for="f in textFields" :key="f.name">
          <dt class="text-muted-foreground">{{ f.name }}</dt>
          <dd>{{ viewing.fields[f.name] ?? "" }}</dd>
        </template>
      </dl>
      <template v-for="f in richFields" :key="f.name">
        <ReadOnlyBody
          v-if="viewing.fields[f.name] && typeof viewing.fields[f.name] === 'object'"
          :doc="viewing.fields[f.name] as never"
          :workspace-id="workspaceId"
          :label="f.name"
          :custom-nodes="customNodes"
        />
      </template>
    </section>

    <Dialog :open="noting !== null" @update:open="(v) => !v && (noting = null)">
      <DialogContent class="max-h-[90svh] overflow-y-auto sm:max-w-xl">
        <DialogHeader>
          <DialogTitle>{{ t("corrections.editNoteTitle", { n: noting?.number ?? 0 }) }}</DialogTitle>
          <DialogDescription>{{ t("corrections.editNoteDescription") }}</DialogDescription>
        </DialogHeader>
        <Textarea v-model="noteText" maxlength="2000" rows="4" :aria-label="t('corrections.note')" />
        <section v-if="noteHistory.length > 0" class="grid gap-2 text-sm">
          <h3 class="font-medium">{{ t("corrections.history") }}</h3>
          <ul class="grid gap-2">
            <li v-for="(h, i) in [...noteHistory].reverse()" :key="i" class="grid gap-0.5 rounded-md border p-2">
              <span class="text-muted-foreground">{{ formatDateTime(h.editedAt, locale) }}</span>
              <span v-if="h.text" class="whitespace-pre-line">{{ h.text }}</span>
              <span v-else class="text-muted-foreground">{{ t("corrections.cleared") }}</span>
            </li>
          </ul>
        </section>
        <FormError :error="noteError" />
        <DialogFooter>
          <Button variant="outline" @click="noting = null">{{ t("common.cancel") }}</Button>
          <Button :disabled="busy" @click="saveNote">{{ t("common.save") }}</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>

    <Dialog :open="erasing !== null" @update:open="(v) => !v && (erasing = null)">
      <DialogContent class="max-h-[90svh] overflow-y-auto sm:max-w-2xl">
        <DialogHeader>
          <DialogTitle>{{ t("versions.eraseTitle", { n: erasing?.number ?? 0 }) }}</DialogTitle>
          <DialogDescription>{{ t("versions.eraseDescription") }}</DialogDescription>
        </DialogHeader>
        <template v-if="erasing?.current">
          <p class="text-sm text-destructive" role="alert">{{ t("versions.eraseCurrent") }}</p>
        </template>
        <div v-else class="grid gap-4 text-sm">
          <p class="rounded-md bg-muted p-3">{{ t("versions.eraseBackups") }}</p>
          <section v-if="candidates && candidates.media.length > 0" class="grid gap-2">
            <h3 class="font-medium">{{ t("versions.eraseMedia") }}</h3>
            <p class="text-muted-foreground">{{ t("versions.eraseMediaHelp") }}</p>
            <ul class="grid gap-2">
              <li v-for="id in candidates.media" :key="id">
                <label class="flex items-center gap-3">
                  <input type="checkbox" :checked="selected.includes(id)" @change="toggle(id, ($event.target as HTMLInputElement).checked)" />
                  <img v-if="media[id]" :src="media[id]!.url" alt="" class="h-12 w-16 rounded object-cover" />
                  <span class="break-all text-muted-foreground">{{ id }}</span>
                </label>
              </li>
            </ul>
          </section>
          <section v-if="candidates && candidates.otherLocales.length > 0" class="grid gap-2">
            <h3 class="font-medium">{{ t("versions.eraseOtherLocales") }}</h3>
            <p class="text-muted-foreground">{{ t("versions.eraseOtherLocalesHelp") }}</p>
            <ul class="flex flex-wrap gap-2">
              <li v-for="o in candidates.otherLocales" :key="o.entryId">
                <NuxtLink :to="`/w/${workspaceId}/entries/${o.entryId}`" class="underline">{{ o.locale }}</NuxtLink>
              </li>
            </ul>
          </section>
        </div>
        <FormError :error="eraseError" />
        <DialogFooter>
          <Button variant="outline" @click="erasing = null">{{ t("common.cancel") }}</Button>
          <Button v-if="!erasing?.current" variant="destructive" :disabled="busy || !candidates" data-testid="confirm-erase" @click="erase">
            {{ t("versions.eraseConfirm") }}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  </div>
</template>
