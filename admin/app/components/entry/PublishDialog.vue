<script setup lang="ts">
// 公開の確認画面（03-content-and-editing.md、Q68, Q75）。
// - 変更されたセクション（アンカーIDごとの比較）と、数値・日付・リンクが変わったセクションへの「誤りの訂正ですか？」のヒント
// - 代替テキストのない画像（あれば公開できない）
// - URLの変更
// 判定はサーバー（publish-check）が、公開と同じコードで行う。確認した下書きのリビジョン番号で公開するので、
// 確認の後に誰かが下書きを変えていれば公開されない（409）。
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import type { PublishCheck, Published } from "~/utils/content";

const props = defineProps<{
  workspaceId: string;
  entryId: string;
  /** 訂正として公開するか。 */
  correction: boolean;
  /** 未保存の変更を保存する（保存できなければ false）。 */
  flush: () => Promise<boolean>;
}>();
const open = defineModel<boolean>("open", { required: true });
const emit = defineEmits<{ published: [result: Published] }>();
const { t } = useI18n();
const api = useApi();

const check = ref<PublishCheck | null>(null);
const loading = ref(false);
const publishing = ref(false);
const error = ref<unknown>(null);
const notSaved = ref(false);

const base = computed(() => `/workspaces/${props.workspaceId}/entries/${props.entryId}`);

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
    const result = await api.post<Published>(`${base.value}/publish`, { revision: check.value.revision, correction: props.correction });
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
        <DialogTitle>{{ correction ? t("publish.correctionTitle") : t("publish.title") }}</DialogTitle>
        <DialogDescription>{{ correction ? t("publish.correctionDescription") : t("publish.description") }}</DialogDescription>
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

        <p v-if="correction && !check.canCorrect" class="text-destructive">{{ t("errors.nothing_to_correct") }}</p>
      </div>

      <DialogFooter>
        <Button variant="outline" @click="open = false">{{ t("common.cancel") }}</Button>
        <Button :disabled="blocked || publishing" data-testid="confirm-publish" @click="publish">
          {{ correction ? t("publish.confirmCorrection") : t("publish.confirm") }}
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
