<script setup lang="ts">
// 原文の差分（03-content-and-editing.md「下書き・公開・版」、Q155, Q162, Q164）。
// 元の記事の、前回「反映済み」にした版から今の版までの差分を、セクションごとに前と後で並べる（変わった行に色を付ける）。
// 文字列はふつうのテキストとして出す（v-html は使わない）。
// 「原文の更新を反映済みにする」は執筆者以上（照合はサーバー）。鮮度の「確認済み」とは別の操作。
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { type SourceDiff, lineDiff } from "~/utils/content";

const props = defineProps<{ workspaceId: string; entryId: string; canReflect: boolean }>();
const open = defineModel<boolean>("open", { required: true });
const emit = defineEmits<{ reflected: [] }>();
const { t } = useI18n();
const api = useApi();
const base = computed(() => `/workspaces/${props.workspaceId}/entries/${props.entryId}/translation`);

const data = ref<SourceDiff | null>(null);
const loading = ref(false);
const error = ref<unknown>(null);
async function load() {
  loading.value = true;
  error.value = null;
  data.value = null;
  try {
    data.value = await api.get<SourceDiff>(`${base.value}/source-diff`);
  } catch (e) {
    error.value = e;
  } finally {
    loading.value = false;
  }
}
watch(open, (v) => v && load(), { immediate: true });

const sections = computed(() =>
  (data.value?.diff?.sections ?? []).map((s) => ({ ...s, lines: lineDiff(s.before, s.after) })),
);
function sectionName(s: { anchor: string | null; title: string | null; field?: string }): string {
  if (!s.anchor) return t("publish.lead", { field: s.field ?? "" });
  return s.title?.trim() || s.anchor;
}
// 文字の色は変えず、背景と左の線で示す（明るい・暗いテーマのどちらでも読めるように）
const lineClass = (kind: string) =>
  kind === "removed" ? "border-l-2 border-destructive bg-destructive/15" : kind === "added" ? "border-l-2 border-emerald-500 bg-emerald-500/15" : "border-l-2 border-transparent";

const reflecting = ref(false);
async function reflect() {
  reflecting.value = true;
  error.value = null;
  try {
    await api.post(`${base.value}/reflected`);
    await refreshNuxtData(translationInfoKey(props.entryId));
    emit("reflected");
    open.value = false;
  } catch (e) {
    error.value = e;
  } finally {
    reflecting.value = false;
  }
}
</script>

<template>
  <Dialog v-model:open="open">
    <DialogContent class="max-h-[90svh] overflow-y-auto sm:max-w-4xl" data-testid="source-diff">
      <DialogHeader>
        <DialogTitle>{{ t("sourceDiff.title") }}</DialogTitle>
        <DialogDescription>{{ t("sourceDiff.description") }}</DialogDescription>
      </DialogHeader>

      <p v-if="loading" class="text-sm text-muted-foreground">{{ t("sourceDiff.loading") }}</p>
      <FormError :error="error" />

      <div v-if="data" class="grid gap-5 text-sm">
        <p v-if="data.unavailable" class="text-muted-foreground" data-testid="source-diff-unavailable">{{ t(`sourceDiff.unavailable.${data.unavailable}`) }}</p>
        <template v-else-if="data.diff">
          <p v-if="data.diff.changedFields.length === 0 && sections.length === 0" class="text-muted-foreground">{{ t("sourceDiff.noChanges") }}</p>
          <section v-if="data.diff.changedFields.length > 0" class="grid gap-1">
            <h3 class="font-medium">{{ t("sourceDiff.changedFields") }}</h3>
            <p>{{ data.diff.changedFields.join(", ") }}</p>
          </section>
          <section v-for="s in sections" :key="`${s.anchor}:${s.field}`" class="grid gap-2">
            <h3 class="flex items-center gap-2 font-medium">
              <Badge variant="outline">{{ t(`publish.kind.${s.kind}`) }}</Badge>
              <span>{{ sectionName(s) }}</span>
            </h3>
            <div class="grid gap-2 md:grid-cols-2">
              <div class="grid content-start gap-1 rounded-md border p-2">
                <p class="text-xs text-muted-foreground">{{ t("sourceDiff.before") }}</p>
                <p v-if="s.before === null" class="text-muted-foreground">{{ t("sourceDiff.none") }}</p>
                <p v-for="(l, i) in s.lines.before" :key="i" class="whitespace-pre-wrap break-words rounded px-1" :class="lineClass(l.kind)">{{ l.text || " " }}</p>
              </div>
              <div class="grid content-start gap-1 rounded-md border p-2">
                <p class="text-xs text-muted-foreground">{{ t("sourceDiff.after") }}</p>
                <p v-if="s.after === null" class="text-muted-foreground">{{ t("sourceDiff.none") }}</p>
                <p v-for="(l, i) in s.lines.after" :key="i" class="whitespace-pre-wrap break-words rounded px-1" :class="lineClass(l.kind)">{{ l.text || " " }}</p>
              </div>
            </div>
          </section>
        </template>
      </div>

      <DialogFooter>
        <Button variant="outline" @click="open = false">{{ t("common.close") }}</Button>
        <Button v-if="canReflect && data && data.unavailable !== 'no_source'" :disabled="reflecting" data-testid="mark-reflected" @click="reflect">
          {{ t("sourceDiff.reflect") }}
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
