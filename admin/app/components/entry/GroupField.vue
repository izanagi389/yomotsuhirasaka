<script setup lang="ts">
// グループのフィールドの入力欄。件の追加・削除・並べ替えを受け持ち、件の中の入力欄は呼び出し元がスロットで描く。
//
// 新しい件には、ここで固定のIDを振る（サーバーと同じ形。groups.rs）。サーバーは正しい形のIDをそのまま残すので、
// 保存のたびにIDが変わらない（IDは引用のアンカーになる。Q10）。
import { Button } from "@/components/ui/button";
import { DEFAULT_GROUP_MAX_ITEMS } from "~/utils/content";
import type { Field, FieldValue, GroupItem } from "~/utils/content";
import { newItemId } from "~/utils/fieldValues";

const props = defineProps<{
  id: string;
  field: Field;
  modelValue: FieldValue | undefined;
  editable: boolean;
}>();
const emit = defineEmits<{ "update:modelValue": [value: FieldValue] }>();
defineSlots<{ item(props: { item: GroupItem; index: number; set: (name: string, value: FieldValue) => void }): unknown }>();
const { t } = useI18n();

const items = computed<GroupItem[]>(() =>
  Array.isArray(props.modelValue) ? props.modelValue.filter((x): x is GroupItem => typeof x === "object" && x !== null) : [],
);
const max = computed(() => props.field.maxItems ?? DEFAULT_GROUP_MAX_ITEMS);

function commit(next: GroupItem[]) {
  emit("update:modelValue", next.length > 0 ? next : null);
}
function add() {
  const used = new Set(items.value.map((i) => i.id));
  let id = newItemId();
  while (used.has(id)) id = newItemId();
  commit([...items.value, { id }]);
}
function remove(index: number) {
  commit(items.value.filter((_, i) => i !== index));
}
function move(index: number, by: -1 | 1) {
  const next = [...items.value];
  const [item] = next.splice(index, 1);
  if (item) next.splice(index + by, 0, item);
  commit(next);
}
function setter(index: number) {
  return (name: string, value: FieldValue) =>
    commit(items.value.map((item, i) => (i === index ? { ...item, [name]: value } : item)));
}
</script>

<template>
  <div :id="id" class="grid gap-3">
    <section
      v-for="(item, index) in items"
      :key="item.id ?? index"
      class="grid gap-3 rounded-md border p-3"
      :aria-label="t('entries.group.item', { n: index + 1 })"
    >
      <div class="flex items-center gap-2 text-xs text-muted-foreground">
        <span class="flex-1">{{ t("entries.group.item", { n: index + 1 }) }}</span>
        <template v-if="editable">
          <Button type="button" variant="ghost" size="sm" :disabled="index === 0" @click="move(index, -1)">{{ t("entries.group.up") }}</Button>
          <Button type="button" variant="ghost" size="sm" :disabled="index === items.length - 1" @click="move(index, 1)">
            {{ t("entries.group.down") }}
          </Button>
          <Button type="button" variant="ghost" size="sm" @click="remove(index)">{{ t("common.remove") }}</Button>
        </template>
      </div>
      <slot name="item" :item="item" :index="index" :set="setter(index)" />
    </section>
    <p v-if="items.length === 0" class="text-sm text-muted-foreground">{{ t("entries.group.empty") }}</p>
    <div v-if="editable" class="flex items-center gap-3">
      <Button type="button" variant="outline" size="sm" :disabled="items.length >= max" @click="add">{{ t("entries.group.add") }}</Button>
      <span class="text-xs text-muted-foreground">{{ t("entries.group.count", { n: items.length, max }) }}</span>
    </div>
  </div>
</template>
