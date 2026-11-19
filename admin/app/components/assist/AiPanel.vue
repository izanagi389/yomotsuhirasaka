<script setup lang="ts">
// 記事の編集画面の右の欄の「AI」タブ（10-llm-and-budget.md「編集者向けのAI支援」、2026-11-17）。
// - 見出しの形の検査（AI を使わない。編集中の本文をずっと見る）を一番上に置く。
// - その下に、ボタンで始める AI の支援（構成の確認、校正、要約とタイトルの案、代替テキストの案、見直しの手がかり）をまとめる。どれも保存済みの下書きが対象で、
//   （校正の提案も）提案を出すだけ（記事は、編集者が当てるまで変わらない）。始める前に、書きかけの下書きを保存する。外部に送られることの注意も、ここに1か所で出す。
import AltTextList from "~/components/assist/AltTextList.vue";
import DraftingPanel from "~/components/assist/DraftingPanel.vue";
import HeadingCheck from "~/components/assist/HeadingCheck.vue";
import ProofreadPanel from "~/components/assist/ProofreadPanel.vue";
import StaleHintsPanel from "~/components/assist/StaleHintsPanel.vue";
import StructurePanel from "~/components/assist/StructurePanel.vue";

defineProps<{
  workspaceId: string;
  entryId: string;
  docs: unknown[];
  editable: boolean;
  /** いま当てられるか（編集のロックを持ち、保存が止まっていない）。 */
  canApply: boolean;
  flush?: () => Promise<unknown>;
  /** コンテンツタイプの titleField・summaryField（なければ、その案の欄を出さない）。 */
  titleField?: string;
  summaryField?: string;
}>();
const emit = defineEmits<{ applyField: [field: string, text: string] }>();
const { t } = useI18n();
</script>

<template>
  <div class="grid gap-6 text-sm" data-testid="ai-panel">
    <HeadingCheck :docs="docs" />
    <template v-if="editable">
      <p class="rounded-md bg-muted px-3 py-2 text-xs text-muted-foreground">{{ t("assist.aiTabNote") }}</p>
      <StructurePanel :workspace-id="workspaceId" :entry-id="entryId" :flush="flush" />
      <ProofreadPanel :workspace-id="workspaceId" :entry-id="entryId" :flush="flush" />
      <DraftingPanel v-if="summaryField" :workspace-id="workspaceId" :entry-id="entryId" kind="summary" :field="summaryField" :can-apply="canApply" :flush="flush" @apply="(f, v) => emit('applyField', f, v)" />
      <DraftingPanel v-if="titleField" :workspace-id="workspaceId" :entry-id="entryId" kind="title" :field="titleField" :can-apply="canApply" :flush="flush" @apply="(f, v) => emit('applyField', f, v)" />
      <AltTextList :docs="docs" :can-apply="canApply" />
      <StaleHintsPanel :workspace-id="workspaceId" :entry-id="entryId" />
    </template>
  </div>
</template>
