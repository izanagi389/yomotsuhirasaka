<script setup lang="ts">
// 翻訳（03-content-and-editing.md「下書き・公開・版」、Q154, Q159）。
// 同じ翻訳グループの記事と、どの記事を元にしたかを表示し、まだないロケールの翻訳を作る（元の記事を複製する）。
// 原文の更新の知らせと差分は M2。
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import type { Draft, TranslationInfo } from "~/utils/content";

const props = defineProps<{
  workspaceId: string;
  entryId: string;
  locales: string[];
  canCreate: boolean;
  /** 未保存の変更を保存する（複製の前に）。 */
  flush: () => Promise<boolean>;
}>();
const { t } = useI18n();
const api = useApi();
const base = computed(() => `/workspaces/${props.workspaceId}/entries/${props.entryId}`);

const { data: info, error, refresh } = useAsyncData(`translation:${props.entryId}`, () => api.get<TranslationInfo>(`${base.value}/translation`));
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
  </div>
</template>
