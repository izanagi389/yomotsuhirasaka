<script setup lang="ts">
// ワークスペースのメディア（01-architecture.md「メディア」、13-security.md「メディア」）。
// - アップロードは執筆者以上。削除は管理者・編集者とサーバー管理者（Q200）
// - 記事で使っているメディアは、使っている記事を見せて確かめてから削除する（サーバーが 409 を返し、force で削除）
import { Upload } from "@lucide/vue";

import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import type { Limits, MediaDetail, MediaInfo } from "~/utils/content";

const route = useRoute();
const ws = route.params.ws as string;
const { t, locale } = useI18n();
const api = useApi();
const { confirm } = useConfirm();
const { workspace, error: wsError } = await useWorkspace(ws);
const { data: items, error, refresh } = await useAsyncData(`media:${ws}`, () => api.get<MediaInfo[]>(`/workspaces/${ws}/media`));
const { data: limits, refresh: refreshLimits } = await useAsyncData(`limits:${ws}`, () => api.get<Limits>(`/workspaces/${ws}/limits`));

// アップロード
const fileInput = ref<HTMLInputElement | null>(null);
const uploading = ref(false);
const actionError = ref<unknown>(null);
async function upload(ev: Event) {
  const input = ev.target as HTMLInputElement;
  const files = [...(input.files ?? [])];
  input.value = "";
  uploading.value = true;
  actionError.value = null;
  try {
    for (const f of files) await api.upload(`/workspaces/${ws}/media`, f, f.type || "application/octet-stream");
  } catch (e) {
    actionError.value = e;
  } finally {
    uploading.value = false;
    await Promise.all([refresh(), refreshLimits()]);
  }
}

// 詳細と削除
const detail = ref<MediaDetail | null>(null);
const confirmDelete = ref(false);
const inUse = ref(false);
const busy = ref(false);
async function open(m: MediaInfo) {
  actionError.value = null;
  confirmDelete.value = false;
  inUse.value = false;
  try {
    detail.value = await api.get<MediaDetail>(`/workspaces/${ws}/media/${m.id}`);
  } catch (e) {
    actionError.value = e;
  }
}
async function remove(force: boolean) {
  if (!detail.value) return;
  const ok = await confirm({
    title: t("confirmations.mediaDelete.title"),
    description: t(force ? "confirmations.mediaDelete.inUseDescription" : "confirmations.mediaDelete.description"),
    confirmLabel: t(force ? "media.deleteAnyway" : "media.delete"),
    destructive: true,
  });
  if (!ok || !detail.value) return;
  busy.value = true;
  actionError.value = null;
  try {
    await api.del(`/workspaces/${ws}/media/${detail.value.id}${force ? "?force=true" : ""}`);
    detail.value = null;
    await Promise.all([refresh(), refreshLimits()]);
  } catch (e) {
    if (e instanceof ApiError && e.code === "media_in_use") inUse.value = true;
    else actionError.value = e;
  } finally {
    busy.value = false;
  }
}
</script>

<template>
  <div class="grid gap-6">
    <PageHeader :title="t('nav.media')" :description="workspace?.name" />
    <div class="flex flex-wrap items-center gap-3 text-sm">
      <span v-if="limits" class="text-muted-foreground">
        {{ t("media.usage", { used: formatBytes(limits.mediaBytesUsed), quota: formatBytes(limits.mediaBytes) }) }}
      </span>
      <template v-if="canEdit(workspace)">
        <Button class="ml-auto" :disabled="uploading" @click="fileInput?.click()"><Upload class="size-4" />{{ t("media.upload") }}</Button>
        <input ref="fileInput" type="file" accept="image/*" multiple class="hidden" @change="upload" />
      </template>
    </div>
    <FormError :error="wsError ?? error ?? (detail ? null : actionError)" />
    <p v-if="items && items.length === 0" class="text-muted-foreground">{{ t("media.empty") }}</p>
    <ul class="grid grid-cols-2 gap-4 sm:grid-cols-3 lg:grid-cols-5">
      <li v-for="m in items ?? []" :key="m.id">
        <button type="button" class="grid w-full gap-1 rounded-md border p-2 text-left text-xs hover:bg-muted/40" :aria-label="t('media.details')" @click="open(m)">
          <span class="flex h-28 items-center justify-center overflow-hidden rounded bg-muted">
            <img :src="m.url" alt="" loading="lazy" class="max-h-28 max-w-full object-contain" />
          </span>
          <span class="text-muted-foreground">{{ t("media.size", { w: m.width, h: m.height, bytes: formatBytes(m.bytes) }) }}</span>
        </button>
      </li>
    </ul>

    <Dialog :open="detail !== null" @update:open="(v) => !v && (detail = null)">
      <DialogContent v-if="detail" class="max-h-[90svh] overflow-y-auto sm:max-w-xl">
        <DialogHeader>
          <DialogTitle>{{ t("media.details") }}</DialogTitle>
          <DialogDescription>{{ t("media.size", { w: detail.width, h: detail.height, bytes: formatBytes(detail.bytes) }) }} · {{ formatDateTime(detail.createdAt, locale) }}</DialogDescription>
        </DialogHeader>
        <div class="flex justify-center rounded bg-muted p-2">
          <img :src="detail.url" alt="" class="max-h-64 max-w-full object-contain" />
        </div>
        <p class="break-all text-xs text-muted-foreground">{{ detail.url }}</p>
        <section class="grid gap-1 text-sm">
          <h3 class="font-medium">{{ t("media.usedBy") }}</h3>
          <p v-if="detail.usage.length === 0" class="text-muted-foreground">{{ t("media.notUsed") }}</p>
          <ul v-else class="grid gap-1">
            <li v-for="u in detail.usage" :key="u.entryId" class="flex gap-2">
              <NuxtLink :to="`/w/${ws}/entries/${u.entryId}`" class="underline">{{ u.locale }}</NuxtLink>
              <span class="text-muted-foreground">
                {{ [u.inDraft ? t("media.inDraft") : null, u.versions.length > 0 ? t("media.inVersions", { versions: u.versions.join(", ") }) : null].filter(Boolean).join(" · ") }}
              </span>
            </li>
          </ul>
        </section>
        <p v-if="inUse" class="text-sm text-destructive" role="alert">{{ t("media.inUse") }}</p>
        <FormError :error="actionError" />
        <DialogFooter v-if="canPublish(workspace)">
          <Button v-if="inUse" variant="destructive" :disabled="busy" @click="remove(true)">{{ t("media.deleteAnyway") }}</Button>
          <Button v-else variant="destructive" :disabled="busy" @click="remove(false)">{{ t("media.delete") }}</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  </div>
</template>
