<script setup lang="ts">
// 見出しの形の検査（構成の確認の a。AI を使わず、書いている間ずっと出す。2026-11-17）。
import { headingProblems, headingsOf } from "~/utils/headings";

const props = defineProps<{ docs: unknown[] }>();
const { t } = useI18n();
const count = computed(() => headingsOf(props.docs).length);
const problems = computed(() => headingProblems(props.docs));
</script>

<template>
  <section class="grid gap-2" data-testid="heading-check">
    <h3 class="font-medium">{{ t("assist.headingsTitle") }}</h3>
    <p v-if="count === 0" class="text-muted-foreground">{{ t("assist.headingsNone") }}</p>
    <p v-else-if="problems.length === 0" class="text-muted-foreground">{{ t("assist.headingsOk") }}</p>
    <ul v-else class="grid list-disc gap-1 pl-5 text-amber-700 dark:text-amber-400">
      <li v-for="(p, i) in problems" :key="i" data-testid="heading-problem">
        <template v-if="p.kind === 'empty'">{{ t("assist.headingEmpty") }}</template>
        <template v-else-if="p.kind === 'skip'">{{ t("assist.headingSkip", { text: p.text, from: p.from, to: p.to }) }}</template>
        <template v-else>{{ t("assist.headingDuplicate", { text: p.text, count: p.count }) }}</template>
      </li>
    </ul>
  </section>
</template>
