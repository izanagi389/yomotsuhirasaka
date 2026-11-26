<script setup lang="ts">
// 選択肢のフィールドの入力欄。1つだけ選ぶならドロップダウン、複数ならチェックボックス。
import type { Field, FieldValue } from "~/utils/content";
import { optionLabel } from "~/utils/labels";

const props = defineProps<{
  id: string;
  field: Field;
  modelValue: FieldValue | undefined;
  editable: boolean;
}>();
const emit = defineEmits<{ "update:modelValue": [value: FieldValue] }>();
const { t, locale: uiLocale } = useI18n();

const options = computed(() => (props.field.options ?? []).map((o) => ({ value: o.value, label: optionLabel(o, uiLocale.value) })));
const selected = computed<string[]>(() => {
  const v = props.modelValue;
  if (Array.isArray(v)) return v.filter((x): x is string => typeof x === "string");
  return typeof v === "string" ? [v] : [];
});

function toggle(value: string, checked: boolean) {
  // 選んだ順ではなく、テンプレートの順に並べる
  const next = options.value.map((o) => o.value).filter((v) => (v === value ? checked : selected.value.includes(v)));
  emit("update:modelValue", next.length > 0 ? next : null);
}
</script>

<template>
  <fieldset v-if="field.multiple" :id="id" class="grid gap-1.5" :disabled="!editable">
    <label v-for="o in options" :key="o.value" class="flex items-center gap-2 text-sm">
      <input
        type="checkbox"
        class="size-4 accent-primary"
        :checked="selected.includes(o.value)"
        @change="toggle(o.value, ($event.target as HTMLInputElement).checked)"
      />
      {{ o.label }}
    </label>
  </fieldset>
  <select
    v-else
    :id="id"
    class="h-9 rounded-md border bg-background px-3 text-sm"
    :value="selected[0] ?? ''"
    :disabled="!editable"
    @change="emit('update:modelValue', ($event.target as HTMLSelectElement).value || null)"
  >
    <option value="">{{ t("entries.chooseOption") }}</option>
    <option v-for="o in options" :key="o.value" :value="o.value">{{ o.label }}</option>
  </select>
</template>
