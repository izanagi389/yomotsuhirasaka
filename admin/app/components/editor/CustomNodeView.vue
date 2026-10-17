<script setup lang="ts">
// 本文のカスタムノード（テンプレートの宣言から作ったもの）。カードの中に、宣言した属性の入力欄を型ごとに並べる。
// 子のあるノードは、カードの中で子を編集できる。表示の見た目はサイト側が決めるので、ここでは入力だけを扱う（原則1）。
import type { CustomAttrDecl, CustomNodeDecl } from "@new-cms/editor-schema";
import { NodeViewContent, NodeViewWrapper, nodeViewProps } from "@tiptap/vue-3";

import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";

const props = defineProps(nodeViewProps);
const { t } = useI18n();

const options = computed(() => props.extension.options as { decl: CustomNodeDecl; isEditable: () => boolean });
const decl = computed(() => options.value.decl);
const editable = computed(() => options.value.isEditable());
const inline = computed(() => decl.value.inline === true);
const hasChildren = computed(() => (decl.value.children ?? []).length > 0);
const idPrefix = `custom-${Math.random().toString(36).slice(2)}`;

const value = (a: CustomAttrDecl) => props.node.attrs[a.name] as string | number | boolean | null;
function setText(a: CustomAttrDecl, v: string | number) {
  props.updateAttributes({ [a.name]: String(v) === "" ? null : String(v) });
}
function setNumber(a: CustomAttrDecl, v: string | number) {
  const n = Number(v);
  props.updateAttributes({ [a.name]: String(v).trim() === "" || !Number.isFinite(n) ? null : n });
}
function setBoolean(a: CustomAttrDecl, v: boolean) {
  props.updateAttributes({ [a.name]: v });
}
</script>

<template>
  <NodeViewWrapper
    :as="inline ? 'span' : 'div'"
    :class="inline ? 'mx-0.5 inline-flex items-center gap-1 rounded border px-1.5 py-0.5 align-baseline text-sm' : 'my-4 rounded-md border p-3'"
    data-testid="custom-node"
    :data-node="decl.name"
  >
    <component :is="inline ? 'span' : 'div'" :class="inline ? 'inline-flex items-center gap-1' : 'grid gap-2'" contenteditable="false" data-custom-form>
      <span class="font-medium">{{ decl.name }}</span>
      <span v-if="!inline && decl.description" class="text-sm text-muted-foreground">{{ decl.description }}</span>
      <component
        :is="inline ? 'span' : 'div'"
        v-for="a in decl.attrs ?? []"
        :key="a.name"
        :class="inline ? 'inline-flex items-center gap-1' : 'grid gap-1'"
      >
        <Label :for="`${idPrefix}-${a.name}`" :class="inline ? 'text-xs' : ''">{{ a.name }}</Label>
        <Switch
          v-if="a.type === 'boolean'"
          :id="`${idPrefix}-${a.name}`"
          :model-value="value(a) === true"
          :disabled="!editable"
          @update:model-value="(v: boolean) => setBoolean(a, v)"
        />
        <Input
          v-else
          :id="`${idPrefix}-${a.name}`"
          :type="a.type === 'number' ? 'number' : 'text'"
          :class="inline ? 'h-6 w-28 text-xs' : ''"
          :model-value="typeof value(a) === 'boolean' ? '' : (value(a) as string | number | null) ?? ''"
          :disabled="!editable"
          :aria-description="a.description"
          @update:model-value="(v: string | number) => (a.type === 'number' ? setNumber(a, v) : setText(a, v))"
        />
        <span v-if="!inline && a.description" class="text-xs text-muted-foreground">{{ a.description }}</span>
      </component>
      <span v-if="(decl.attrs ?? []).length === 0 && !hasChildren" class="text-sm text-muted-foreground">{{ t("editor.custom.noAttrs") }}</span>
    </component>
    <NodeViewContent v-if="hasChildren" :as="inline ? 'span' : 'div'" class="mt-2 rounded border border-dashed p-2" />
  </NodeViewWrapper>
</template>
