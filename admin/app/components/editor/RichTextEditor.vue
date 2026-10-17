<script setup lang="ts">
// 本文のエディタ（Tiptap）。ワークスペースのマニフェストにあるノードとマークだけを使う。
// - 本文に、このエディタが知らないノードやマークがあれば、編集させない（読み込むと消えてしまうため）。
// - 保存するJSONからは、マニフェストにない属性を取り除く（サーバーは定義にない属性を拒否する）。
// - Tiptap が差し込む <style> は CSP で使えないので、injectCSS を切り、assets/css/editor.css に置いている。
import type { JSONContent } from "@tiptap/core";
import { NodeSelection } from "@tiptap/pm/state";
import { EditorContent, useEditor } from "@tiptap/vue-3";
import { type ManifestBody, forManifest, sanitize, unknownTypes } from "@new-cms/editor-schema";
import {
  Bold,
  Clapperboard,
  Code,
  Heading2,
  Heading3,
  Heading4,
  ImagePlus,
  Italic,
  Link,
  List,
  ListOrdered,
  Minus,
  Pilcrow,
  Puzzle,
  Quote,
  Redo2,
  SquareCode,
  Strikethrough,
  Undo2,
} from "@lucide/vue";

import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { editorExtensions } from "./extensions";
import type { MediaInfo } from "~/utils/content";

const props = defineProps<{
  modelValue: JSONContent | null;
  body: ManifestBody;
  workspaceId: string;
  editable: boolean;
  label: string;
}>();
const emit = defineEmits<{ "update:modelValue": [value: JSONContent] }>();
const { t } = useI18n();
const api = useApi();

// 編集できるか（ノードビューにも渡す）
const editableRef = ref(false);
const narrowed = forManifest(
  editorExtensions({
    workspaceId: props.workspaceId,
    embedProviders: props.body.embedProviders ?? [],
    isEditable: () => editableRef.value,
    customNodes: props.body.customNodes ?? [],
  }),
  props.body,
);
const unknown = props.modelValue ? unknownTypes(props.modelValue, narrowed.extensions) : [];
const canEdit = computed(() => props.editable && narrowed.editable && unknown.length === 0);
watchEffect(() => (editableRef.value = canEdit.value));

const editor = useEditor({
  extensions: narrowed.extensions,
  content: props.modelValue ?? undefined,
  editable: canEdit.value,
  injectCSS: false,
  editorProps: { attributes: { "aria-label": props.label, "aria-multiline": "true", role: "textbox" } },
  onUpdate: ({ editor }) => emit("update:modelValue", sanitize(editor.getJSON(), props.body)),
});
// 切り替えで update を出さない（出すと、内容が変わっていないのに自動保存が走る）
watch(canEdit, (v) => editor.value?.setEditable(v, false));

const has = (kind: "nodes" | "marks", name: string) => !!editor.value?.schema[kind][name];
const active = (name: string, attrs?: Record<string, unknown>) => !!editor.value?.isActive(name, attrs);
const chain = () => editor.value!.chain().focus();

interface Tool {
  key: string;
  icon: Component;
  label: string;
  show: boolean;
  active?: boolean;
  run: () => void;
}
const tools = computed<Tool[][]>(() => {
  if (!editor.value) return [];
  return [
    [
      { key: "paragraph", icon: Pilcrow, label: t("editor.tools.paragraph"), show: true, active: active("paragraph"), run: () => chain().setParagraph().run() },
      ...([2, 3, 4] as const).map((level) => ({
        key: `h${level}`,
        icon: { 2: Heading2, 3: Heading3, 4: Heading4 }[level],
        label: t("editor.tools.heading", { level }),
        show: has("nodes", "heading"),
        active: active("heading", { level }),
        run: () => chain().toggleHeading({ level }).run(),
      })),
    ],
    [
      { key: "bold", icon: Bold, label: t("editor.tools.bold"), show: has("marks", "bold"), active: active("bold"), run: () => chain().toggleBold().run() },
      { key: "italic", icon: Italic, label: t("editor.tools.italic"), show: has("marks", "italic"), active: active("italic"), run: () => chain().toggleItalic().run() },
      { key: "strike", icon: Strikethrough, label: t("editor.tools.strike"), show: has("marks", "strike"), active: active("strike"), run: () => chain().toggleStrike().run() },
      { key: "code", icon: Code, label: t("editor.tools.code"), show: has("marks", "code"), active: active("code"), run: () => chain().toggleCode().run() },
      { key: "link", icon: Link, label: t("editor.tools.link"), show: has("marks", "link"), active: active("link"), run: openLink },
    ],
    [
      { key: "bulletList", icon: List, label: t("editor.tools.bulletList"), show: has("nodes", "bulletList"), active: active("bulletList"), run: () => chain().toggleBulletList().run() },
      { key: "orderedList", icon: ListOrdered, label: t("editor.tools.orderedList"), show: has("nodes", "orderedList"), active: active("orderedList"), run: () => chain().toggleOrderedList().run() },
      { key: "blockquote", icon: Quote, label: t("editor.tools.blockquote"), show: has("nodes", "blockquote"), active: active("blockquote"), run: () => chain().toggleBlockquote().run() },
      { key: "codeBlock", icon: SquareCode, label: t("editor.tools.codeBlock"), show: has("nodes", "codeBlock"), active: active("codeBlock"), run: () => chain().toggleCodeBlock().run() },
      { key: "hr", icon: Minus, label: t("editor.tools.horizontalRule"), show: has("nodes", "horizontalRule"), run: () => chain().setHorizontalRule().run() },
    ],
    [
      { key: "image", icon: ImagePlus, label: t("editor.tools.image"), show: has("nodes", "image"), run: () => fileInput.value?.click() },
      { key: "embed", icon: Clapperboard, label: t("editor.tools.embed"), show: has("nodes", "embed"), run: openEmbed },
    ],
    [
      { key: "undo", icon: Undo2, label: t("editor.tools.undo"), show: true, run: () => chain().undo().run() },
      { key: "redo", icon: Redo2, label: t("editor.tools.redo"), show: true, run: () => chain().redo().run() },
    ],
  ].map((group) => group.filter((tool) => tool.show));
});

// リンク
const linkOpen = ref(false);
const linkHref = ref("");
const linkInvalid = ref(false);
function openLink() {
  linkHref.value = (editor.value?.getAttributes("link").href as string | undefined) ?? "";
  linkInvalid.value = false;
  linkOpen.value = true;
}
function applyLink() {
  const href = linkHref.value.trim();
  if (!href) {
    chain().extendMarkRange("link").unsetLink().run();
  } else if (!hrefAllowed(href)) {
    linkInvalid.value = true;
    return;
  } else {
    chain().extendMarkRange("link").setLink({ href }).run();
  }
  linkOpen.value = false;
}

// 埋め込み
const embedOpen = ref(false);
const embedSrc = ref("");
const embedInvalid = ref(false);
function openEmbed() {
  embedSrc.value = "";
  embedInvalid.value = false;
  embedOpen.value = true;
}
function applyEmbed() {
  const src = embedSrc.value.trim();
  if (!embedAllowed(src, props.body.embedProviders ?? [])) {
    embedInvalid.value = true;
    return;
  }
  chain().insertContent({ type: "embed", attrs: { src } }).run();
  embedOpen.value = false;
}

// 画像（アップロードして挿入する。代替テキストはその場で入れる）
const fileInput = ref<HTMLInputElement | null>(null);
const uploading = ref(false);
const uploadError = ref<unknown>(null);
async function upload(ev: Event) {
  const input = ev.target as HTMLInputElement;
  const file = input.files?.[0];
  input.value = "";
  if (!file) return;
  uploading.value = true;
  uploadError.value = null;
  try {
    const media = await api.upload<MediaInfo>(`/workspaces/${props.workspaceId}/media`, file, file.type || "application/octet-stream");
    chain().insertContent({ type: "image", attrs: { mediaId: media.id } }).run();
  } catch (e) {
    uploadError.value = e;
  } finally {
    uploading.value = false;
  }
}

// カスタムノード（テンプレートの宣言）。カーソルの位置に挿入する
const customNodes = computed(() => (props.body.customNodes ?? []).filter((c) => has("nodes", c.name)));
function insertCustom(name: string) {
  // ノード全体が選ばれているとき（画像やカスタムノードを選んだ状態）は、置き換えずに、その後ろに入れる
  const { selection } = editor.value!.state;
  if (selection instanceof NodeSelection) chain().insertContentAt(selection.to, { type: name }).run();
  else chain().insertContent({ type: name }).run();
}

onBeforeUnmount(() => editor.value?.destroy());
</script>

<template>
  <div class="grid gap-2">
    <div v-if="unknown.length > 0 || !narrowed.editable" class="rounded-md border border-amber-500/50 bg-amber-500/10 p-3 text-sm" role="status">
      {{ t("editor.readonlyUnknown", { names: (unknown.length > 0 ? unknown : narrowed.unsupported).join(", ") || "paragraph" }) }}
    </div>
    <div class="cms-editor rounded-md border">
      <div v-if="canEdit && editor" class="flex flex-wrap items-center gap-1 border-b p-1" role="toolbar" :aria-label="t('editor.toolbar')">
        <template v-for="(group, i) in tools" :key="i">
          <span v-if="i > 0 && group.length > 0" class="mx-1 h-5 w-px bg-border" aria-hidden="true" />
          <Button
            v-for="tool in group"
            :key="tool.key"
            type="button"
            variant="ghost"
            size="icon"
            class="size-8"
            :class="tool.active ? 'bg-accent text-accent-foreground' : ''"
            :aria-label="tool.label"
            :aria-pressed="tool.active ?? undefined"
            :title="tool.label"
            :disabled="tool.key === 'image' && uploading"
            @mousedown.prevent
            @click="tool.run"
          >
            <component :is="tool.icon" class="size-4" />
          </Button>
        </template>
        <template v-if="customNodes.length > 0">
          <span class="mx-1 h-5 w-px bg-border" aria-hidden="true" />
          <DropdownMenu>
            <DropdownMenuTrigger as-child>
              <Button type="button" variant="ghost" size="icon" class="size-8" :aria-label="t('editor.custom.menu')" :title="t('editor.custom.menu')" data-testid="insert-custom-node" @mousedown.prevent>
                <Puzzle class="size-4" />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="start">
              <DropdownMenuItem v-for="c in customNodes" :key="c.name" :data-testid="`insert-custom-node-${c.name}`" @select="insertCustom(c.name)">
                <span class="grid">
                  <span>{{ c.name }}</span>
                  <span v-if="c.description" class="text-xs text-muted-foreground">{{ c.description }}</span>
                </span>
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        </template>
        <input ref="fileInput" type="file" accept="image/*" class="hidden" @change="upload" />
      </div>
      <EditorContent :editor="editor" class="px-4 py-3" />
    </div>
    <FormError :error="uploadError" />

    <Dialog v-model:open="linkOpen">
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{{ t("editor.link.title") }}</DialogTitle>
          <DialogDescription>{{ t("editor.link.description") }}</DialogDescription>
        </DialogHeader>
        <form class="grid gap-2" @submit.prevent="applyLink">
          <Label for="link-href">{{ t("editor.link.href") }}</Label>
          <Input id="link-href" v-model="linkHref" placeholder="https://" autocomplete="off" />
          <p v-if="linkInvalid" class="text-sm text-destructive">{{ t("editor.link.invalid") }}</p>
          <DialogFooter>
            <Button type="button" variant="outline" @click="linkOpen = false">{{ t("common.cancel") }}</Button>
            <Button type="submit">{{ linkHref.trim() ? t("editor.link.apply") : t("editor.link.remove") }}</Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>

    <Dialog v-model:open="embedOpen">
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{{ t("editor.embed.title") }}</DialogTitle>
          <DialogDescription>{{ t("editor.embed.description", { providers: (body.embedProviders ?? []).join(", ") || "-" }) }}</DialogDescription>
        </DialogHeader>
        <form class="grid gap-2" @submit.prevent="applyEmbed">
          <Label for="embed-src">{{ t("editor.embed.label") }}</Label>
          <Input id="embed-src" v-model="embedSrc" placeholder="https://" autocomplete="off" />
          <p v-if="embedInvalid" class="text-sm text-destructive">{{ t("editor.embed.notAllowed", { providers: (body.embedProviders ?? []).join(", ") || "-" }) }}</p>
          <DialogFooter>
            <Button type="button" variant="outline" @click="embedOpen = false">{{ t("common.cancel") }}</Button>
            <Button type="submit">{{ t("editor.embed.insert") }}</Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  </div>
</template>
