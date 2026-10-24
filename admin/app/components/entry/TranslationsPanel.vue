<script setup lang="ts">
// 翻訳（03-content-and-editing.md「下書き・公開・版」、Q154, Q159）。
// 同じ翻訳グループの記事と、どの記事を元にしたかを表示し、まだないロケールの翻訳を作る（元の記事を複製する）。
// 原文の更新があれば知らせ、差分を開ける（Q159）。元の記事は、編集者以上が同じグループの別の記事に設定し直せる（Q156）。
// 設定し直すと反映済みの版はその記事の今の版になるので、確認してから送る。
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Label } from "@/components/ui/label";
import type { Draft } from "~/utils/content";

const props = defineProps<{
  workspaceId: string;
  entryId: string;
  locales: string[];
  canCreate: boolean;
  /** 元の記事を設定し直せるか（編集者以上）。 */
  canSetSource: boolean;
  /** 未保存の変更を保存する（複製の前に）。 */
  flush: () => Promise<boolean>;
}>();
const { t } = useI18n();
const api = useApi();
const base = computed(() => `/workspaces/${props.workspaceId}/entries/${props.entryId}`);

const emit = defineEmits<{ showDiff: [] }>();
const { data: info, error, refresh } = useTranslationInfo(props.workspaceId, props.entryId);
const missing = computed(() => props.locales.filter((l) => !info.value?.members.some((m) => m.locale === l)));

const creating = ref<string | null>(null);
const createError = ref<unknown>(null);
async function create(locale: string) {
  creating.value = locale;
  createError.value = null;
  try {
    // 複製するのはサーバーの下書きなので、先に保存しておく
    if (!(await props.flush())) throw new Error("notSaved");
    const draft = await api.post<Draft>(`${base.value}/translations`, { locale });
    await navigateTo(`/w/${props.workspaceId}/entries/${draft.id}`);
  } catch (e) {
    createError.value = e;
    await refresh();
  } finally {
    creating.value = null;
  }
}

// 元の記事の設定し直し
const candidates = computed(() => (info.value?.members ?? []).filter((m) => m.entryId !== props.entryId && m.entryId !== info.value?.sourceEntryId));
const newSource = ref("");
const confirmSource = ref(false);
const settingSource = ref(false);
const sourceError = ref<unknown>(null);
async function setSource() {
  settingSource.value = true;
  sourceError.value = null;
  try {
    await api.put(`${base.value}/translation/source`, { sourceEntryId: newSource.value });
    newSource.value = "";
    confirmSource.value = false;
    await refresh();
  } catch (e) {
    sourceError.value = e;
  } finally {
    settingSource.value = false;
  }
}
const localeOf = (id: string) => info.value?.members.find((m) => m.entryId === id)?.locale ?? id;
</script>

<template>
  <div class="grid gap-6 text-sm">
    <FormError :error="error ?? createError" />
    <template v-if="info">
      <section class="grid gap-2">
        <h2 class="font-medium">{{ t("translations.source") }}</h2>
        <p v-if="info.sourceEntryId">
          <NuxtLink :to="`/w/${workspaceId}/entries/${info.sourceEntryId}`" class="underline">{{ info.sourceLocale }}</NuxtLink>
          <span class="text-muted-foreground"> — {{ info.reflectedSourceVersionId ? t("translations.reflected") : t("translations.reflectedNone") }}</span>
        </p>
        <p v-else class="text-muted-foreground">{{ t("translations.noSource") }}</p>
        <div v-if="info.sourceEntryId" class="flex flex-wrap items-center gap-3">
          <span v-if="info.sourceUpdate" class="text-amber-700 dark:text-amber-400" data-testid="translations-source-update">
            {{ info.sourceUpdate.corrected ? t("sourceUpdate.corrected") : t("sourceUpdate.updated") }}
          </span>
          <span v-else class="text-muted-foreground">{{ t("sourceUpdate.upToDate") }}</span>
          <Button size="sm" variant="outline" @click="emit('showDiff')">{{ t("sourceUpdate.showDiff") }}</Button>
        </div>
        <div v-if="canSetSource && candidates.length > 0" class="flex flex-wrap items-end gap-2">
          <div class="grid gap-1.5">
            <Label for="translation-source">{{ t("translations.setSource") }}</Label>
            <select id="translation-source" v-model="newSource" class="h-9 rounded-md border bg-background px-3" data-testid="translation-source">
              <option value="" disabled>{{ t("translations.chooseSource") }}</option>
              <option v-for="m in candidates" :key="m.entryId" :value="m.entryId">{{ m.locale }}</option>
            </select>
          </div>
          <Button size="sm" variant="outline" :disabled="!newSource" @click="confirmSource = true">{{ t("translations.setSourceButton") }}</Button>
        </div>
        <FormError :error="sourceError" />
      </section>

      <section class="grid gap-2">
        <h2 class="font-medium">{{ t("translations.members") }}</h2>
        <ul class="grid gap-1">
          <li v-for="m in info.members" :key="m.entryId" class="flex items-center gap-2">
            <Badge variant="outline">{{ m.locale }}</Badge>
            <span v-if="m.entryId === entryId" class="text-muted-foreground">{{ t("translations.thisEntry") }}</span>
            <NuxtLink v-else :to="`/w/${workspaceId}/entries/${m.entryId}`" class="underline">{{ t("translations.open") }}</NuxtLink>
            <Badge :variant="m.published ? 'default' : 'secondary'">{{ m.published ? t("entries.published") : t("entries.draft") }}</Badge>
          </li>
        </ul>
      </section>

      <section v-if="canCreate && missing.length > 0" class="grid gap-2">
        <h2 class="font-medium">{{ t("translations.create") }}</h2>
        <p class="text-muted-foreground">{{ t("translations.createHelp") }}</p>
        <div class="flex flex-wrap gap-2">
          <Button v-for="l in missing" :key="l" variant="outline" size="sm" :disabled="creating !== null" @click="create(l)">
            {{ t("translations.createFor", { locale: l }) }}
          </Button>
        </div>
      </section>
    </template>

    <Dialog v-model:open="confirmSource">
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{{ t("translations.setSourceTitle") }}</DialogTitle>
          <DialogDescription>{{ t("translations.setSourceDescription", { locale: localeOf(newSource) }) }}</DialogDescription>
        </DialogHeader>
        <DialogFooter>
          <Button variant="outline" @click="confirmSource = false">{{ t("common.cancel") }}</Button>
          <Button :disabled="settingSource" data-testid="confirm-set-source" @click="setSource">{{ t("translations.setSourceButton") }}</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  </div>
</template>
