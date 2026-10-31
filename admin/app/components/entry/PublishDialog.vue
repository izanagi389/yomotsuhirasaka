<script setup lang="ts">
// 公開の確認画面（03-content-and-editing.md、Q68, Q75）。
// - 変更されたセクション（アンカーIDごとの比較）と、数値・日付・リンクが変わったセクションへの「誤りの訂正ですか？」のヒント
// - 代替テキストのない画像（あれば公開できない）
// - URLの変更
// 判定はサーバー（publish-check）が、公開と同じコードで行う。確認した下書きのリビジョン番号で公開するので、
// 確認の後に誰かが下書きを変えていれば公開されない（409）。
// 訂正として公開するときは、訂正の説明を定型文と自由記述で書ける（任意。後から版のタブで書き直せる。Q63）。
// 予約公開（Q78）では、同じ確認の後に公開の時刻を選ぶ。予約のときにサーバーが公開と同じ検査と計算を済ませ、その時点の下書きを
// 予約版として固定する（訂正は予約できない）。時刻はブラウザのタイムゾーンで入力し、UTC で送る。
// 翻訳の記事で原文に未確認の更新があれば知らせ、その場で差分を見て「反映済み」にできる。公開は止めない（Q162, Q164）。
// AI支援が有効なら、意味の近い他の記事との食い違いを確かめる（ConflictPanel。M4）。結果を待たずに公開できる（INV-BUD-02）。
import { Input } from "@/components/ui/input";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import SourceDiffDialog from "~/components/entry/SourceDiffDialog.vue";
import ConflictPanel from "~/components/assist/ConflictPanel.vue";
import type { CorrectionTemplate, PublishCheck, Published } from "~/utils/content";

const props = defineProps<{
  workspaceId: string;
  entryId: string;
  /** 記事のロケール（訂正の説明の定型文を選ぶ）。 */
  locale: string;
  /** 訂正として公開するか。 */
  correction: boolean;
  /** 予約公開か（訂正とは一緒に使わない）。 */
  schedule?: boolean;
  /** 未保存の変更を保存する（保存できなければ false）。 */
  flush: () => Promise<boolean>;
  /** 「原文の更新を反映済み」にできるか（執筆者以上）。 */
  canReflect?: boolean;
}>();
const open = defineModel<boolean>("open", { required: true });
const emit = defineEmits<{ published: [result: Published]; scheduled: [] }>();

// 予約の時刻（datetime-local の値。既定は1時間後の分の切り捨て）
const when = ref("");
const timeZone = Intl.DateTimeFormat().resolvedOptions().timeZone;
function defaultWhen(): string {
  const d = new Date(Date.now() + 60 * 60 * 1000);
  d.setSeconds(0, 0);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}`;
}
watch(open, (v) => v && props.schedule && (when.value = defaultWhen()), { immediate: true });
const { t } = useI18n();
const api = useApi();

const check = ref<PublishCheck | null>(null);
const diffOpen = ref(false);
function onReflected() {
  if (check.value) check.value = { ...check.value, sourceUpdate: null };
}
const loading = ref(false);
const publishing = ref(false);
const error = ref<unknown>(null);
const notSaved = ref(false);

const base = computed(() => `/workspaces/${props.workspaceId}/entries/${props.entryId}`);

// 訂正の説明
const choices = ref<CorrectionTemplate[]>([]);
const templateId = ref("");
const noteText = ref("");
async function loadTemplates() {
  templateId.value = "";
  noteText.value = "";
  try {
    const all = await api.get<CorrectionTemplate[]>(`/workspaces/${props.workspaceId}/correction-templates`);
    choices.value = all.filter((c) => c.kind === "choice" && c.locale === props.locale);
  } catch {
    // 定型文が読めなくても、自由記述で書ける
    choices.value = [];
  }
}
watch(open, (v) => v && props.correction && loadTemplates(), { immediate: true });
const correctionNote = computed(() =>
  templateId.value || noteText.value.trim() ? { templateId: templateId.value || undefined, text: noteText.value.trim() || undefined } : undefined,
);

async function load() {
  loading.value = true;
  error.value = null;
  notSaved.value = false;
  check.value = null;
  try {
    if (!(await props.flush())) {
      notSaved.value = true;
      return;
    }
    check.value = await api.get<PublishCheck>(`${base.value}/publish-check`);
  } catch (e) {
    error.value = e;
  } finally {
    loading.value = false;
  }
}
watch(open, (v) => v && load(), { immediate: true });

const blocked = computed(() => {
  const c = check.value;
  if (!c) return true;
  return c.problems.length > 0 || c.imagesMissingAlt.length > 0 || c.urlTaken || !c.url || (props.correction && !c.canCorrect);
});
const hinted = computed(() => (check.value?.sections ?? []).filter((s) => s.numbersOrLinksChanged));

async function publish() {
  if (!check.value) return;
  publishing.value = true;
  error.value = null;
  try {
    if (props.schedule) {
      await api.put(`${base.value}/schedule`, { revision: check.value.revision, publishAt: new Date(when.value).toISOString() });
      open.value = false;
      emit("scheduled");
      return;
    }
    const result = await api.post<Published>(`${base.value}/publish`, {
      revision: check.value.revision,
      correction: props.correction,
      ...(props.correction && correctionNote.value ? { correctionNote: correctionNote.value } : {}),
    });
    open.value = false;
    emit("published", result);
  } catch (e) {
    error.value = e;
    // 確認の後に下書きが変わった: 確認し直す
    if (e instanceof ApiError && e.code === "revision_conflict") await load();
  } finally {
    publishing.value = false;
  }
}

/** `fields.body.content[2]` を、人が読める場所にする（フィールドの名前と、何番目の画像か）。 */
function imagePlace(path: string, index: number): string {
  const field = /^fields\.([^.[]+)/.exec(path)?.[1] ?? "";
  return t("publish.imagePlace", { field, n: index + 1 });
}
/** 公開を止める問題の文。必須の欠けは画面の言語で、それ以外はサーバーの文のまま。 */
function problemText(p: string): string {
  const field = /^fields\.([^:]+): required to publish$/.exec(p)?.[1];
  return field ? t("publish.required", { field }) : p;
}
function sectionName(s: { anchor: string | null; title: string | null; field: string | null }): string {
  if (!s.anchor) return t("publish.lead", { field: s.field ?? "" });
  return s.title?.trim() || s.anchor;
}
</script>

<template>
  <Dialog v-model:open="open">
    <DialogContent class="max-h-[90svh] overflow-y-auto sm:max-w-2xl">
      <DialogHeader>
        <DialogTitle>{{ schedule ? t("schedule.title") : correction ? t("publish.correctionTitle") : t("publish.title") }}</DialogTitle>
        <DialogDescription>{{ schedule ? t("schedule.description") : correction ? t("publish.correctionDescription") : t("publish.description") }}</DialogDescription>
      </DialogHeader>

      <p v-if="loading" class="text-sm text-muted-foreground">{{ t("publish.checking") }}</p>
      <p v-if="notSaved" class="text-sm text-destructive" role="alert">{{ t("publish.notSaved") }}</p>
      <FormError :error="error" />

      <div v-if="check" class="grid gap-5 text-sm" data-testid="publish-check">
        <section v-if="check.problems.length > 0" class="grid gap-1">
          <h3 class="font-medium text-destructive">{{ t("publish.problems") }}</h3>
          <ul class="list-disc pl-5">
            <li v-for="p in check.problems" :key="p">{{ problemText(p) }}</li>
          </ul>
        </section>

        <section v-if="check.imagesMissingAlt.length > 0" class="grid gap-1" data-testid="missing-alt">
          <h3 class="font-medium text-destructive">{{ t("publish.missingAlt") }}</h3>
          <p class="text-muted-foreground">{{ t("publish.missingAltHelp") }}</p>
          <ul class="list-disc pl-5">
            <li v-for="(p, i) in check.imagesMissingAlt" :key="p">{{ imagePlace(p, i) }}</li>
          </ul>
        </section>

        <section class="grid gap-1">
          <h3 class="font-medium">{{ t("publish.url") }}</h3>
          <p v-if="check.url" class="break-all">{{ check.url }}</p>
          <p v-else class="text-destructive">{{ t("publish.noUrl") }}</p>
          <p v-if="check.previousUrl" class="text-muted-foreground">{{ t("publish.urlChanged", { from: check.previousUrl }) }}</p>
          <p v-if="check.urlTaken" class="text-destructive">{{ t("errors.url_taken") }}</p>
        </section>

        <section v-if="check.comparedWith !== null" class="grid gap-2" data-testid="changed-sections">
          <h3 class="font-medium">{{ t("publish.changedSections", { n: check.comparedWith }) }}</h3>
          <p v-if="check.sections === null" class="text-muted-foreground">{{ t("publish.erased") }}</p>
          <p v-else-if="check.sections.length === 0" class="text-muted-foreground">{{ t("publish.noChanges") }}</p>
          <ul v-else class="grid gap-1">
            <li v-for="s in check.sections" :key="`${s.anchor}:${s.field}`" class="flex items-center gap-2">
              <Badge variant="outline">{{ t(`publish.kind.${s.kind}`) }}</Badge>
              <span>{{ sectionName(s) }}</span>
            </li>
          </ul>
          <p v-if="!correction && hinted.length > 0" class="rounded-md bg-muted p-3 text-muted-foreground" data-testid="correction-hint">
            {{ t("publish.correctionHint", { sections: hinted.map(sectionName).join(", ") }) }}
          </p>
        </section>
        <p v-else class="text-muted-foreground">{{ t("publish.firstTime") }}</p>

        <section v-if="check.sourceUpdate" class="grid gap-2 rounded-md border border-amber-500/50 bg-amber-500/10 p-3" data-testid="publish-source-update">
          <p>{{ check.sourceUpdate.corrected ? t("sourceUpdate.publishCorrected") : t("sourceUpdate.publishUpdated") }}</p>
          <div>
            <Button size="sm" variant="outline" @click="diffOpen = true">{{ t("sourceUpdate.showDiff") }}</Button>
          </div>
        </section>

        <ConflictPanel :workspace-id="workspaceId" :entry-id="entryId" />

        <p v-if="correction && !check.canCorrect" class="text-destructive">{{ t("errors.nothing_to_correct") }}</p>

        <section v-if="correction && check.canCorrect" class="grid gap-3" data-testid="correction-note">
          <h3 class="font-medium">{{ t("corrections.note") }}</h3>
          <p class="text-muted-foreground">{{ t("corrections.noteHelp") }}</p>
          <div v-if="choices.length > 0" class="grid gap-1.5">
            <Label for="correction-template">{{ t("corrections.template") }}</Label>
            <select id="correction-template" v-model="templateId" class="h-9 rounded-md border bg-transparent px-3">
              <option value="">{{ t("corrections.noTemplate") }}</option>
              <option v-for="c in choices" :key="c.id" :value="c.id">{{ c.text }}</option>
            </select>
          </div>
          <div class="grid gap-1.5">
            <Label for="correction-text">{{ t("corrections.freeText") }}</Label>
            <Textarea id="correction-text" v-model="noteText" maxlength="2000" rows="3" />
          </div>
        </section>
      </div>

      <section v-if="schedule && check" class="grid gap-1.5 text-sm" data-testid="schedule-time">
        <Label for="schedule-at">{{ t("schedule.at") }}</Label>
        <Input id="schedule-at" v-model="when" type="datetime-local" class="w-fit" required />
        <p class="text-muted-foreground">{{ t("schedule.atHelp", { zone: timeZone }) }}</p>
      </section>

      <DialogFooter>
        <Button variant="outline" @click="open = false">{{ t("common.cancel") }}</Button>
        <Button :disabled="blocked || publishing || (schedule && !when)" data-testid="confirm-publish" @click="publish">
          {{ schedule ? t("schedule.confirm") : correction ? t("publish.confirmCorrection") : t("publish.confirm") }}
        </Button>
      </DialogFooter>
      <!-- 差分のダイアログは、確認画面のダイアログの中に入れ子にする（フォーカスの扱いのため） -->
      <SourceDiffDialog v-model:open="diffOpen" :workspace-id="workspaceId" :entry-id="entryId" :can-reflect="canReflect ?? false" @reflected="onReflected" />
    </DialogContent>
  </Dialog>
</template>
