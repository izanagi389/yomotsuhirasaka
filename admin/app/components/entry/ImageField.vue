<script setup lang="ts">
// 画像のフィールド（サムネイルなど。02-schema-and-delivery-api.md「画像のフィールド」）。
// - ワークスペースのメディアから選ぶか、その場でアップロードする。1枚だけ
// - 本文の画像と同じく、代替テキストか「装飾画像」の印がないと公開できない（INV-IMG-01）
// - 削除したメディアや他のワークスペースのメディアは、サーバーが保存を拒否する（INV-IMG-03）
import { ImagePlus, Upload, X } from "@lucide/vue";

import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { mediaInfo } from "~/components/editor/media";
import type { ImageValue, MediaInfo } from "~/utils/content";

const props = defineProps<{
  id: string;
  modelValue: ImageValue | null;
  workspaceId: string;
  editable: boolean;
}>();
const emit = defineEmits<{ "update:modelValue": [value: ImageValue | null] }>();
const { t } = useI18n();
const api = useApi();

const media = ref<MediaInfo | null>(null);
const failed = ref(false);
watch(
  () => props.modelValue?.mediaId ?? null,
  async (mediaId) => {
    media.value = null;
    failed.value = false;
    if (!mediaId) return;
    const info = await mediaInfo(props.workspaceId, mediaId);
    if (props.modelValue?.mediaId === mediaId) {
      media.value = info;
      failed.value = !info;
    }
  },
  { immediate: true },
);

const alt = computed(() => props.modelValue?.alt ?? "");
const decorative = computed(() => props.modelValue?.decorative === true);
const missing = computed(() => !!props.modelValue && !decorative.value && alt.value.trim() === "");

function update(patch: Partial<ImageValue>) {
  if (!props.modelValue) return;
  const next: ImageValue = { ...props.modelValue, ...patch };
  if (!next.alt) delete next.alt;
  if (!next.decorative) delete next.decorative;
  emit("update:modelValue", next);
}
function choose(m: MediaInfo) {
  // 選び直しても、書いた代替テキストと印は残す
  emit("update:modelValue", { ...(props.modelValue ?? {}), mediaId: m.id });
  picking.value = false;
}

// メディアから選ぶ
const picking = ref(false);
const library = ref<MediaInfo[] | null>(null);
const libraryError = ref<unknown>(null);
async function openPicker() {
  picking.value = true;
  libraryError.value = null;
  try {
    library.value = await api.get<MediaInfo[]>(`/workspaces/${props.workspaceId}/media`);
  } catch (e) {
    libraryError.value = e;
  }
}

// その場でアップロード
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
    choose(await api.upload<MediaInfo>(`/workspaces/${props.workspaceId}/media`, file, file.type || "application/octet-stream"));
  } catch (e) {
    uploadError.value = e;
  } finally {
    uploading.value = false;
  }
}
// 欄の名前のラベル（EntryField）は代替テキストの入力欄を指す
const altId = props.id;
</script>

<template>
  <div class="grid gap-3 rounded-md border p-3" :class="missing ? 'border-destructive' : ''" data-testid="image-field">
    <div class="flex flex-col gap-3 sm:flex-row">
      <div class="flex h-32 w-full shrink-0 items-center justify-center overflow-hidden rounded bg-muted sm:w-48">
        <img v-if="media" :src="media.url" :alt="decorative ? '' : alt" class="max-h-32 max-w-full object-contain" />
        <span v-else-if="failed" class="text-sm text-muted-foreground">{{ t("editor.image.notFound") }}</span>
        <span v-else-if="!modelValue" class="text-sm text-muted-foreground">{{ t("entries.image.none") }}</span>
      </div>
      <div class="grid flex-1 content-start gap-2 text-sm">
        <template v-if="modelValue">
          <label :for="altId" class="font-medium">{{ t("editor.image.alt") }}</label>
          <input
            :id="altId"
            class="h-9 rounded-md border bg-background px-3 disabled:opacity-50"
            :value="alt"
            :disabled="decorative || !editable"
            :placeholder="t('editor.image.altPlaceholder')"
            @input="update({ alt: ($event.target as HTMLInputElement).value })"
          />
          <label class="flex items-center gap-2">
            <input type="checkbox" :checked="decorative" :disabled="!editable" @change="update({ decorative: ($event.target as HTMLInputElement).checked })" />
            {{ t("editor.image.decorative") }}
          </label>
          <p v-if="missing" class="text-destructive">{{ t("editor.image.missing") }}</p>
        </template>
        <div v-if="editable" class="flex flex-wrap gap-2">
          <Button type="button" variant="outline" size="sm" @click="openPicker"><ImagePlus class="size-4" />{{ t("entries.image.choose") }}</Button>
          <Button type="button" variant="outline" size="sm" :disabled="uploading" @click="fileInput?.click()"><Upload class="size-4" />{{ t("entries.image.upload") }}</Button>
          <Button v-if="modelValue" type="button" variant="ghost" size="sm" @click="emit('update:modelValue', null)"><X class="size-4" />{{ t("entries.image.remove") }}</Button>
          <input ref="fileInput" type="file" accept="image/png,image/jpeg,image/gif,image/webp" class="hidden" @change="upload" />
        </div>
        <FormError :error="uploadError" />
      </div>
    </div>

    <Dialog v-model:open="picking">
      <DialogContent class="max-h-[90svh] overflow-y-auto sm:max-w-2xl">
        <DialogHeader>
          <DialogTitle>{{ t("entries.image.choose") }}</DialogTitle>
          <DialogDescription>{{ t("entries.image.chooseHint") }}</DialogDescription>
        </DialogHeader>
        <FormError :error="libraryError" />
        <p v-if="library && library.length === 0" class="text-sm text-muted-foreground">{{ t("media.empty") }}</p>
        <ul v-if="library" class="grid grid-cols-2 gap-2 sm:grid-cols-4">
          <li v-for="m in library" :key="m.id">
            <button
              type="button"
              class="grid w-full gap-1 rounded-md border p-1 hover:bg-muted/40"
              :class="m.id === modelValue?.mediaId ? 'ring-2 ring-primary' : ''"
              :aria-label="t('entries.image.pick', { w: m.width, h: m.height })"
              @click="choose(m)"
            >
              <img :src="m.url" alt="" class="h-24 w-full rounded object-cover" loading="lazy" />
              <span class="text-xs text-muted-foreground">{{ m.width }}×{{ m.height }}</span>
            </button>
          </li>
        </ul>
      </DialogContent>
    </Dialog>
  </div>
</template>
