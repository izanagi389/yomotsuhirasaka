<script setup lang="ts">
// 記事のフィールドの入力欄（マニフェストのフィールドの型ごと）。
import type { JSONContent } from "@tiptap/core";

import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import RichTextEditor from "~/components/editor/RichTextEditor.vue";
import type { EntrySummary, Field, FieldValue, Manifest } from "~/utils/content";

const props = defineProps<{
  field: Field;
  modelValue: FieldValue | undefined;
  manifest: Manifest;
  workspaceId: string;
  locale: string;
  editable: boolean;
  /** 参照フィールドの候補（ワークスペースの記事の一覧）。 */
  entries: EntrySummary[];
}>();
const emit = defineEmits<{ "update:modelValue": [value: FieldValue] }>();
const { t } = useI18n();
const id = `field-${props.field.name}`;

const text = computed(() => (typeof props.modelValue === "string" ? props.modelValue : ""));
function setText(v: string | number) {
  const s = String(v);
  emit("update:modelValue", s === "" ? null : s);
}

// 参照: 記事ではなく翻訳グループを指す（Q157）。候補は、参照先のコンテンツタイプの記事を翻訳グループごとにまとめ、
// この記事と同じロケールの記事の名前（なければ他のロケールのもの）で表示する。
const referenceType = computed(() => props.manifest.contentTypes.find((c) => c.name === props.field.referenceTo));
const candidates = computed(() => {
  const groups = new Map<string, EntrySummary>();
  for (const e of props.entries.filter((e) => e.contentType === props.field.referenceTo)) {
    const current = groups.get(e.translationGroupId);
    if (!current || (e.locale === props.locale && current.locale !== props.locale)) groups.set(e.translationGroupId, e);
  }
  return [...groups.entries()].map(([group, e]) => ({
    group,
    label: `${entryTitle(referenceType.value, e.fields) ?? t("entries.untitled")} (${e.locale})`,
  }));
});
const labelOf = (group: string) => candidates.value.find((c) => c.group === group)?.label ?? group;
const selected = computed<string[]>(() => {
  const v = props.modelValue;
  if (Array.isArray(v)) return v;
  return typeof v === "string" ? [v] : [];
});
function addReference(group: string) {
  if (!group) return;
  if (props.field.multiple) {
    if (!selected.value.includes(group)) emit("update:modelValue", [...selected.value, group]);
  } else {
    emit("update:modelValue", group);
  }
}
function removeReference(group: string) {
  if (props.field.multiple) {
    const rest = selected.value.filter((g) => g !== group);
    emit("update:modelValue", rest.length > 0 ? rest : null);
  } else {
    emit("update:modelValue", null);
  }
}
</script>

<template>
  <div class="grid gap-2">
    <Label :for="id">
      {{ field.name }}
      <span v-if="field.required || field.type === 'slug'" class="text-destructive" :title="t('entries.requiredToPublish')">*</span>
    </Label>
    <p v-if="field.description" class="text-sm text-muted-foreground">{{ field.description }}</p>

    <Input v-if="field.type === 'text'" :id="id" :model-value="text" :disabled="!editable" @update:model-value="setText" />

    <template v-else-if="field.type === 'slug'">
      <Input :id="id" :model-value="text" :disabled="!editable" autocomplete="off" spellcheck="false" @update:model-value="setText" />
      <p class="text-xs text-muted-foreground">{{ t("entries.slugHint") }}</p>
    </template>

    <RichTextEditor
      v-else-if="field.type === 'richtext'"
      :model-value="(modelValue as JSONContent | null) ?? null"
      :body="manifest.body"
      :workspace-id="workspaceId"
      :editable="editable"
      :label="field.name"
      @update:model-value="emit('update:modelValue', $event)"
    />

    <div v-else-if="field.type === 'reference'" class="grid gap-2">
      <ul v-if="selected.length > 0" class="grid gap-1 text-sm">
        <li v-for="g in selected" :key="g" class="flex items-center gap-2 rounded-md border px-3 py-1.5">
          <span class="flex-1">{{ labelOf(g) }}</span>
          <button v-if="editable" type="button" class="text-muted-foreground hover:text-foreground" @click="removeReference(g)">
            {{ t("common.remove") }}
          </button>
        </li>
      </ul>
      <select
        v-if="editable && (field.multiple || selected.length === 0)"
        :id="id"
        class="h-9 rounded-md border bg-background px-3 text-sm"
        :value="''"
        @change="addReference(($event.target as HTMLSelectElement).value)"
      >
        <option value="">{{ t("entries.chooseReference") }}</option>
        <option v-for="c in candidates.filter((c) => !selected.includes(c.group))" :key="c.group" :value="c.group">{{ c.label }}</option>
      </select>
    </div>
  </div>
</template>
