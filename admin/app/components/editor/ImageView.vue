<script setup lang="ts">
// 本文の画像。代替テキストと「装飾画像」の印をその場で編集する（Q75）。どちらもない画像は公開できない。
import { NodeViewWrapper, nodeViewProps } from "@tiptap/vue-3";

import { mediaInfo } from "./media";
import type { MediaInfo } from "~/utils/content";

const props = defineProps(nodeViewProps);
const { t } = useI18n();

const media = ref<MediaInfo | null>(null);
const failed = ref(false);
const options = computed(() => props.extension.options as { workspaceId: string; isEditable: () => boolean });
const workspaceId = computed(() => options.value.workspaceId);
const editable = computed(() => options.value.isEditable());

watch(
  () => props.node.attrs.mediaId as string | null,
  async (id) => {
    media.value = null;
    failed.value = false;
    if (!id) return;
    const info = await mediaInfo(workspaceId.value, id);
    if (props.node.attrs.mediaId === id) {
      media.value = info;
      failed.value = !info;
    }
  },
  { immediate: true },
);

const alt = computed(() => (props.node.attrs.alt as string | null) ?? "");
const decorative = computed(() => props.node.attrs.decorative === true);
const missing = computed(() => !decorative.value && alt.value.trim() === "");
const inputId = `alt-${Math.random().toString(36).slice(2)}`;
</script>

<template>
  <NodeViewWrapper class="my-4 rounded-md border p-3" :class="missing ? 'border-destructive' : ''" data-testid="image-node">
    <div class="flex flex-col gap-3 sm:flex-row" contenteditable="false">
      <div class="flex h-32 w-full shrink-0 items-center justify-center overflow-hidden rounded bg-muted sm:w-48">
        <img v-if="media" :src="media.url" :alt="decorative ? '' : alt" class="max-h-32 max-w-full object-contain" draggable="false" />
        <span v-else-if="failed" class="text-sm text-muted-foreground">{{ t("editor.image.notFound") }}</span>
      </div>
      <div class="grid flex-1 content-start gap-2 text-sm">
        <label :for="inputId" class="font-medium">{{ t("editor.image.alt") }}</label>
        <input
          :id="inputId"
          class="h-9 rounded-md border bg-background px-3 disabled:opacity-50"
          :value="alt"
          :disabled="decorative || !editable"
          :placeholder="t('editor.image.altPlaceholder')"
          @input="updateAttributes({ alt: ($event.target as HTMLInputElement).value || null })"
        />
        <label class="flex items-center gap-2">
          <input
            type="checkbox"
            :checked="decorative"
            :disabled="!editable"
            @change="updateAttributes({ decorative: ($event.target as HTMLInputElement).checked })"
          />
          {{ t("editor.image.decorative") }}
        </label>
        <p v-if="missing" class="text-destructive">{{ t("editor.image.missing") }}</p>
      </div>
    </div>
  </NodeViewWrapper>
</template>
