<script setup lang="ts">
// 「AI」タブの、代替テキストのない画像の一覧（2026-11-17）。1枚ずつ代替テキストの案を出せる（AltTextSuggest）。
import AltTextSuggest from "~/components/assist/AltTextSuggest.vue";
import { applyAlt, imagesMissingAlt } from "~/utils/altText";

const props = defineProps<{ docs: unknown[]; canApply: boolean }>();
const { t } = useI18n();
const editors = useBodyEditors();
const images = computed(() => imagesMissingAlt(props.docs));
</script>

<template>
  <section class="grid gap-3" data-testid="alt-text-list">
    <h3 class="font-medium">{{ t("assist.altSuggestTitle") }}</h3>
    <p v-if="images.length === 0" class="text-muted-foreground">{{ t("assist.altAllSet") }}</p>
    <template v-else>
      <p class="text-muted-foreground">{{ t("assist.altSuggestHelp") }}</p>
      <div v-for="(id, i) in images" :key="id" class="grid gap-2 rounded-md border p-2">
        <span class="text-xs font-medium">{{ t("assist.altImage", { n: i + 1 }) }}</span>
        <AltTextSuggest :media-id="id" :can-apply="canApply" @apply="(text) => applyAlt(editors, id, text)" />
      </div>
    </template>
  </section>
</template>
