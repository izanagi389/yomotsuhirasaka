<script setup lang="ts">
// 右の欄の「ブロック」タブ（ブロック型のエディタ。03-content-and-editing.md「本文のエディタ」）。
// 選んだブロックの設定を出す: 見出しの階層、画像の代替テキストと装飾画像の印（Q75）、埋め込みの URL、カスタム部品の属性。
// 本文の中には入力欄を出さない（クラシックでは、これまでどおり本文の中で編集する）。
import type { CustomAttrDecl, CustomNodeDecl } from "@yomotsuhirasaka/editor-schema";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { blockTarget, setBlockAttrs, type BlockTarget } from "./blocks";
import { mediaInfo } from "./media";
import type { MediaInfo } from "~/utils/content";

const { t } = useI18n();
const { active, tick } = useBlockSelection();

function extensionOptions(name: string): Record<string, unknown> | undefined {
  return active.value?.extensionManager.extensions.find((e) => e.name === name)?.options as Record<string, unknown> | undefined;
}
const declOf = (name: string) => extensionOptions(name)?.["decl"] as CustomNodeDecl | undefined;

const target = computed<BlockTarget | null>(() => {
  void tick.value;
  const editor = active.value;
  if (!editor || editor.isDestroyed) return null;
  return blockTarget(editor.state, (name) => !!declOf(name));
});
const kind = computed(() => target.value?.node.type.name ?? null);
const attrs = computed(() => target.value?.node.attrs ?? {});
const editable = computed(() => {
  void tick.value;
  return !!active.value?.isEditable;
});
const decl = computed(() => (kind.value ? declOf(kind.value) : undefined));

/** ブロックの種類の名前（ツールバーと同じ文言）。 */
const kindLabel = computed(() => {
  const k = kind.value;
  if (!k) return "";
  if (decl.value) return decl.value.name;
  if (k === "heading") return t("editor.tools.heading", { level: attrs.value["level"] as number });
  const key: Record<string, string> = {
    paragraph: "editor.tools.paragraph",
    bulletList: "editor.tools.bulletList",
    orderedList: "editor.tools.orderedList",
    blockquote: "editor.tools.blockquote",
    codeBlock: "editor.tools.codeBlock",
    horizontalRule: "editor.tools.horizontalRule",
    image: "editor.blockSettings.image",
    embed: "editor.blockSettings.embed",
  };
  return key[k] ? t(key[k]) : k;
});

function set(values: Record<string, unknown>) {
  if (active.value && target.value) setBlockAttrs(active.value, target.value, values);
}

// 見出し
const LEVELS = [2, 3, 4] as const;

// 画像
const media = ref<MediaInfo | null>(null);
const mediaFailed = ref(false);
const workspaceId = computed(() => extensionOptions("image")?.["workspaceId"] as string | undefined);
watch(
  () => (kind.value === "image" ? (attrs.value["mediaId"] as string | null) : null),
  async (id) => {
    media.value = null;
    mediaFailed.value = false;
    if (!id || !workspaceId.value) return;
    const info = await mediaInfo(workspaceId.value, id);
    if (attrs.value["mediaId"] === id) {
      media.value = info;
      mediaFailed.value = !info;
    }
  },
  { immediate: true },
);
const alt = computed(() => (attrs.value["alt"] as string | null) ?? "");
const decorative = computed(() => attrs.value["decorative"] === true);
const altMissing = computed(() => !decorative.value && alt.value.trim() === "");

// 埋め込み
const embedSrc = computed(() => (attrs.value["src"] as string | null) ?? "");
const embedProviders = computed(() => (extensionOptions("embed")?.["embedProviders"] as string[] | undefined) ?? []);

// カスタム部品
const customValue = (a: CustomAttrDecl) => attrs.value[a.name] as string | number | boolean | null;
function setCustomText(a: CustomAttrDecl, v: string | number) {
  set({ [a.name]: String(v) === "" ? null : String(v) });
}
function setCustomNumber(a: CustomAttrDecl, v: string | number) {
  const n = Number(v);
  set({ [a.name]: String(v).trim() === "" || !Number.isFinite(n) ? null : n });
}
</script>

<template>
  <div class="grid gap-4" data-testid="block-settings">
    <p v-if="!target" class="text-muted-foreground">{{ t("editor.blockSettings.empty") }}</p>
    <template v-else>
      <h2 class="text-base font-semibold" data-testid="block-settings-kind">{{ kindLabel }}</h2>

      <div v-if="kind === 'heading'" class="grid gap-2">
        <span class="font-medium">{{ t("editor.blockSettings.level") }}</span>
        <div class="flex gap-1" role="group" :aria-label="t('editor.blockSettings.level')">
          <button
            v-for="l in LEVELS"
            :key="l"
            type="button"
            class="rounded-md border px-3 py-1"
            :class="attrs['level'] === l ? 'border-primary bg-accent font-medium' : ''"
            :aria-pressed="attrs['level'] === l"
            :disabled="!editable"
            :data-testid="`block-heading-${l}`"
            @click="set({ level: l })"
          >
            H{{ l }}
          </button>
        </div>
      </div>

      <div v-else-if="kind === 'image'" class="grid gap-3">
        <div class="flex h-40 items-center justify-center overflow-hidden rounded bg-muted">
          <img v-if="media" :src="media.url" :alt="decorative ? '' : alt" class="max-h-40 max-w-full object-contain" />
          <span v-else-if="mediaFailed" class="text-muted-foreground">{{ t("editor.image.notFound") }}</span>
        </div>
        <div class="grid gap-1">
          <Label for="block-image-alt">{{ t("editor.image.alt") }}</Label>
          <Input
            id="block-image-alt"
            :model-value="alt"
            :disabled="decorative || !editable"
            :placeholder="t('editor.image.altPlaceholder')"
            data-testid="block-image-alt"
            @update:model-value="(v: string | number) => set({ alt: String(v) || null })"
          />
        </div>
        <label class="flex items-center gap-2">
          <Switch :model-value="decorative" :disabled="!editable" data-testid="block-image-decorative" @update:model-value="(v: boolean) => set({ decorative: v })" />
          {{ t("editor.image.decorative") }}
        </label>
        <p v-if="altMissing" class="text-destructive">{{ t("editor.image.missing") }}</p>
      </div>

      <div v-else-if="kind === 'embed'" class="grid gap-1">
        <span class="font-medium">{{ t("editor.embed.label") }}</span>
        <span class="break-all text-muted-foreground">{{ embedSrc || "-" }}</span>
        <p v-if="!embedAllowed(embedSrc, embedProviders)" class="text-destructive">{{ t("editor.embed.notAllowed", { providers: embedProviders.join(", ") || "-" }) }}</p>
        <p class="text-xs text-muted-foreground">{{ t("editor.blockSettings.embedChange") }}</p>
      </div>

      <div v-else-if="decl" class="grid gap-3">
        <p v-if="decl.description" class="text-muted-foreground">{{ decl.description }}</p>
        <div v-for="a in decl.attrs ?? []" :key="a.name" class="grid gap-1">
          <Label :for="`block-attr-${a.name}`">{{ a.name }}</Label>
          <Switch
            v-if="a.type === 'boolean'"
            :id="`block-attr-${a.name}`"
            :model-value="customValue(a) === true"
            :disabled="!editable"
            @update:model-value="(v: boolean) => set({ [a.name]: v })"
          />
          <Input
            v-else
            :id="`block-attr-${a.name}`"
            :type="a.type === 'number' ? 'number' : 'text'"
            :model-value="typeof customValue(a) === 'boolean' ? '' : ((customValue(a) as string | number | null) ?? '')"
            :disabled="!editable"
            :data-testid="`block-attr-${a.name}`"
            @update:model-value="(v: string | number) => (a.type === 'number' ? setCustomNumber(a, v) : setCustomText(a, v))"
          />
          <span v-if="a.description" class="text-xs text-muted-foreground">{{ a.description }}</span>
        </div>
        <p v-if="(decl.attrs ?? []).length === 0" class="text-muted-foreground">{{ t("editor.custom.noAttrs") }}</p>
      </div>

      <p v-else class="text-muted-foreground">{{ t("editor.blockSettings.none") }}</p>
    </template>
  </div>
</template>
