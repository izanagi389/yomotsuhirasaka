<script setup lang="ts">
// 記事の編集画面の右の欄の「AI」タブ（10-llm-and-budget.md「編集者向けのAI支援」、2026-11-17）。
// - 見出しの形の検査（AI を使わない。編集中の本文をずっと見る）を一番上に置く。
// - その下に、ボタンで始める AI の支援（構成の確認、見直しの手がかり）をまとめる。どれも保存済みの下書きが対象で、
//   提案を出すだけ（記事は、編集者が当てるまで変わらない）。始める前に、書きかけの下書きを保存する。外部に送られることの注意も、ここに1か所で出す。
import HeadingCheck from "~/components/assist/HeadingCheck.vue";
import StaleHintsPanel from "~/components/assist/StaleHintsPanel.vue";
import StructurePanel from "~/components/assist/StructurePanel.vue";

defineProps<{ workspaceId: string; entryId: string; docs: unknown[]; editable: boolean; flush?: () => Promise<unknown> }>();
const { t } = useI18n();
</script>

<template>
  <div class="grid gap-6 text-sm" data-testid="ai-panel">
    <HeadingCheck :docs="docs" />
    <template v-if="editable">
      <p class="rounded-md bg-muted px-3 py-2 text-xs text-muted-foreground">{{ t("assist.aiTabNote") }}</p>
      <StructurePanel :workspace-id="workspaceId" :entry-id="entryId" :flush="flush" />
      <StaleHintsPanel :workspace-id="workspaceId" :entry-id="entryId" />
    </template>
  </div>
</template>
