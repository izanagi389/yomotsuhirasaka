<script setup lang="ts">
// 本文の埋め込み。管理画面ではURLだけを表示し、中身は読み込まない。URLは挿入のときに確かめる（変えるときは入れ直す）。
// 許可した提供元の https のURLだけを、サーバーが受け付ける（Q97）。
import { NodeViewWrapper, nodeViewProps } from "@tiptap/vue-3";

const props = defineProps(nodeViewProps);
const { t } = useI18n();

const src = computed(() => (props.node.attrs.src as string | null) ?? "");
const providers = computed(() => (props.extension.options as { embedProviders: string[] }).embedProviders);
const allowed = computed(() => embedAllowed(src.value, providers.value));
</script>

<template>
  <NodeViewWrapper class="my-4 rounded-md border p-3" :class="allowed ? '' : 'border-destructive'">
    <div class="grid gap-1 text-sm" contenteditable="false">
      <span class="font-medium">{{ t("editor.embed.label") }}</span>
      <span class="break-all text-muted-foreground">{{ src || "-" }}</span>
      <p v-if="!allowed" class="text-destructive">{{ t("editor.embed.notAllowed", { providers: providers.join(", ") || "-" }) }}</p>
    </div>
  </NodeViewWrapper>
</template>
