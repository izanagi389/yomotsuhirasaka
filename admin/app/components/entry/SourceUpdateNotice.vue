<script setup lang="ts">
// 原文の更新の知らせ（03-content-and-editing.md、Q159。訂正のときは「原文が訂正されました」、06、Q155）。記事の画面の上に出す。
// 知らせは「原文の更新を反映済み」にするまで消えない（翻訳の記事を公開しても消えない。Q162）。
import { Button } from "@/components/ui/button";

const props = defineProps<{ workspaceId: string; entryId: string }>();
const emit = defineEmits<{ showDiff: [] }>();
const { t } = useI18n();
const { data } = useTranslationInfo(props.workspaceId, props.entryId);
const update = computed(() => data.value?.sourceUpdate ?? null);
</script>

<template>
  <div v-if="update" class="flex flex-wrap items-center gap-3 rounded-md border border-amber-500/50 bg-amber-500/10 p-3 text-sm" role="status" data-testid="source-update">
    <span class="flex-1">{{ update.corrected ? t("sourceUpdate.corrected") : t("sourceUpdate.updated") }}</span>
    <Button size="sm" variant="outline" data-testid="show-source-diff" @click="emit('showDiff')">{{ t("sourceUpdate.showDiff") }}</Button>
  </div>
</template>
