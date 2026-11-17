<script setup lang="ts">
// ブロック型のエディタの、ブロックの左のつまみ（⋮⋮）。
// - マウスを乗せたブロック（乗せていなければ、カーソルのあるブロック）の左に出す
// - つまみをドラッグすると並べ替え（タッチの画面ではドラッグしない）。押すと「上へ・下へ・複製・削除」のメニュー
// - 位置は、本文の欄（container）の中での相対の位置。CSP の style-src に触れないよう、style 属性ではなく CSSOM で動かす
import type { Editor } from "@tiptap/core";
import { ArrowDown, ArrowUp, Copy, GripVertical, Trash2 } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { blockAt, deleteBlock, duplicateBlock, endBlockDrag, moveBlock, selectedBlock, startBlockDrag } from "./blocks";

const props = defineProps<{ editor: Editor; container: HTMLElement | null }>();
const { t } = useI18n();

const hovered = ref<number | null>(null);
const selected = ref<number | null>(null);
const index = computed(() => hovered.value ?? selected.value);
const count = ref(0);
const menuOpen = ref(false);
const handle = ref<HTMLElement | null>(null);
// タッチが主の画面（スマートフォンなど）ではドラッグしない（文字の選択やスクロールとぶつかるため）
const canDrag = import.meta.client && !window.matchMedia("(pointer: coarse)").matches;

/** ブロックの DOM（本文の直下の要素）。 */
function blockDom(i: number | null): HTMLElement | null {
  if (i === null) return null;
  const b = blockAt(props.editor.state.doc, i);
  const dom = b ? props.editor.view.nodeDOM(b.pos) : null;
  return dom instanceof HTMLElement ? dom : null;
}

function place() {
  const el = handle.value;
  const dom = blockDom(index.value);
  if (!el) return;
  if (!dom || !props.container) {
    el.hidden = true;
    return;
  }
  const top = dom.getBoundingClientRect().top - props.container.getBoundingClientRect().top;
  el.hidden = false;
  el.style.setProperty("top", `${Math.max(0, top)}px`);
}

function sync() {
  count.value = props.editor.state.doc.childCount;
  selected.value = props.editor.isFocused ? (selectedBlock(props.editor.state)?.index ?? null) : selected.value;
  if (hovered.value !== null && hovered.value >= count.value) hovered.value = null;
  nextTick(place);
}

function onMove(event: MouseEvent) {
  if (menuOpen.value) return;
  const root = props.editor.view.dom;
  let el = event.target instanceof Element ? event.target : null;
  // つまみの上では、いまのブロックのまま
  if (el && handle.value?.contains(el)) return;
  while (el && el.parentElement !== root) el = el.parentElement;
  if (!el) return;
  const doc = props.editor.state.doc;
  let pos = 0;
  for (let i = 0; i < doc.childCount; i++) {
    if (props.editor.view.nodeDOM(pos) === el) {
      if (hovered.value !== i) {
        hovered.value = i;
        nextTick(place);
      }
      return;
    }
    pos += doc.child(i).nodeSize;
  }
}
function onLeave(event: MouseEvent) {
  if (menuOpen.value || (event.relatedTarget instanceof Node && handle.value?.contains(event.relatedTarget))) return;
  hovered.value = null;
  nextTick(place);
}

watch(
  () => props.container,
  (el, old) => {
    old?.removeEventListener("mousemove", onMove);
    old?.removeEventListener("mouseleave", onLeave);
    el?.addEventListener("mousemove", onMove);
    el?.addEventListener("mouseleave", onLeave);
    nextTick(place);
  },
  { immediate: true },
);
onMounted(() => {
  props.editor.on("transaction", sync);
  props.editor.on("focus", sync);
  window.addEventListener("resize", place);
  sync();
});
onBeforeUnmount(() => {
  props.editor.off("transaction", sync);
  props.editor.off("focus", sync);
  window.removeEventListener("resize", place);
  props.container?.removeEventListener("mousemove", onMove);
  props.container?.removeEventListener("mouseleave", onLeave);
});

function onDragStart(event: DragEvent) {
  if (index.value === null) return;
  startBlockDrag(props.editor, index.value, event, blockDom(index.value));
}
function onDragEnd() {
  endBlockDrag(props.editor);
  hovered.value = null;
  sync();
}

function act(run: (i: number) => void) {
  const i = index.value;
  if (i === null) return;
  run(i);
  hovered.value = null;
  sync();
}
</script>

<template>
  <div ref="handle" class="absolute left-1 z-10" hidden data-testid="block-handle">
    <DropdownMenu v-model:open="menuOpen">
      <DropdownMenuTrigger as-child>
        <Button
          type="button"
          variant="ghost"
          size="icon"
          class="size-7 cursor-grab text-muted-foreground active:cursor-grabbing"
          :aria-label="t('editor.block.menu')"
          :title="t('editor.block.menu')"
          :draggable="canDrag"
          data-testid="block-handle-button"
          @dragstart="onDragStart"
          @dragend="onDragEnd"
        >
          <GripVertical class="size-4" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" side="left">
        <DropdownMenuItem :disabled="index === null || index === 0" data-testid="block-move-up" @select="act((i) => moveBlock(editor, i, -1))">
          <ArrowUp aria-hidden="true" />{{ t("editor.block.moveUp") }}
        </DropdownMenuItem>
        <DropdownMenuItem :disabled="index === null || index >= count - 1" data-testid="block-move-down" @select="act((i) => moveBlock(editor, i, 1))">
          <ArrowDown aria-hidden="true" />{{ t("editor.block.moveDown") }}
        </DropdownMenuItem>
        <DropdownMenuItem data-testid="block-duplicate" @select="act((i) => duplicateBlock(editor, i))"><Copy aria-hidden="true" />{{ t("editor.block.duplicate") }}</DropdownMenuItem>
        <DropdownMenuSeparator />
        <DropdownMenuItem class="text-destructive focus:text-destructive" data-testid="block-delete" @select="act((i) => deleteBlock(editor, i))">
          <Trash2 aria-hidden="true" />{{ t("editor.block.delete") }}
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  </div>
</template>
