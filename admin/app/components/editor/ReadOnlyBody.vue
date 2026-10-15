<script setup lang="ts">
// 本文の表示だけ（版の中身を見るとき）。エディタが知らない要素があれば、消さずに知らせ、文字だけを表示する。
import type { JSONContent } from "@tiptap/core";
import { EditorContent, useEditor } from "@tiptap/vue-3";
import { unknownTypes } from "@new-cms/editor-schema";

import { editorExtensions } from "./extensions";

const props = defineProps<{ doc: JSONContent; workspaceId: string; label: string }>();
const { t } = useI18n();

const extensions = editorExtensions({ workspaceId: props.workspaceId, embedProviders: [], isEditable: () => false });
const unknown = unknownTypes(props.doc, extensions);
const editor = unknown.length === 0
  ? useEditor({
      extensions,
      content: props.doc,
      editable: false,
      injectCSS: false,
      editorProps: { attributes: { "aria-label": props.label, "aria-readonly": "true" } },
    })
  : null;

/** 知らない要素があるときの、文字だけの表示（段落ごと）。 */
const blocks = computed(() => {
  const out: string[] = [];
  const text = (n: JSONContent): string => (n.text ?? "") + (n.content ?? []).map(text).join("");
  for (const n of props.doc.content ?? []) out.push(text(n));
  return out.filter((s) => s.trim() !== "");
});
onBeforeUnmount(() => editor?.value?.destroy());
</script>

<template>
  <div class="cms-editor rounded-md border">
    <EditorContent v-if="editor" :editor="editor" class="px-4 py-3" />
    <div v-else class="grid gap-2 px-4 py-3">
      <p class="text-sm text-muted-foreground" role="status">{{ t("versions.unknownNodes", { names: unknown.join(", ") }) }}</p>
      <p v-for="(b, i) in blocks" :key="i">{{ b }}</p>
    </div>
  </div>
</template>
