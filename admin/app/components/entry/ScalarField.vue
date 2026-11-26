<script setup lang="ts">
// 数値・真偽値・日付・日時のフィールドの入力欄（ブラウザの標準の入力欄と、切り替えのスイッチ）。
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import type { Field, FieldValue } from "~/utils/content";
import { browserTimeZone, fromLocalInput, toLocalInput, toNumber } from "~/utils/fieldValues";

const props = defineProps<{
  id: string;
  field: Field;
  modelValue: FieldValue | undefined;
  editable: boolean;
}>();
const emit = defineEmits<{ "update:modelValue": [value: FieldValue] }>();
const { t } = useI18n();

const numberText = computed(() => (typeof props.modelValue === "number" ? String(props.modelValue) : ""));
const dateText = computed(() => (typeof props.modelValue === "string" ? props.modelValue : ""));
const datetimeText = computed(() => toLocalInput(props.modelValue));
const zone = browserTimeZone();
</script>

<template>
  <Input
    v-if="field.type === 'number'"
    :id="id"
    type="number"
    inputmode="decimal"
    :step="field.integer ? 1 : 'any'"
    :min="field.min"
    :max="field.max"
    :model-value="numberText"
    :readonly="!editable"
    @update:model-value="emit('update:modelValue', toNumber($event))"
  />
  <div v-else-if="field.type === 'boolean'" class="flex items-center gap-2">
    <Switch
      :id="id"
      :model-value="modelValue === true"
      :disabled="!editable"
      @update:model-value="emit('update:modelValue', $event)"
    />
    <span class="text-sm text-muted-foreground">{{ modelValue === true ? t("entries.boolean.on") : t("entries.boolean.off") }}</span>
  </div>
  <Input
    v-else-if="field.type === 'date'"
    :id="id"
    type="date"
    :model-value="dateText"
    :readonly="!editable"
    @update:model-value="emit('update:modelValue', $event === '' ? null : String($event))"
  />
  <template v-else-if="field.type === 'datetime'">
    <Input
      :id="id"
      type="datetime-local"
      step="1"
      :model-value="datetimeText"
      :readonly="!editable"
      @update:model-value="emit('update:modelValue', fromLocalInput(String($event)))"
    />
    <p class="text-xs text-muted-foreground">{{ t("entries.datetimeZone", { zone }) }}</p>
  </template>
</template>
