<script setup lang="ts">
// ワークスペースの記事の一覧（/w/{ws}/entries）。記事の作成もここから行う。ワークスペースの入り口は「やること」（/w/{ws}）。
import { Plus } from "@lucide/vue";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Label } from "@/components/ui/label";
import type { Draft, EntrySummary } from "~/utils/content";

const route = useRoute();
const ws = route.params.ws as string;
const { t, locale } = useI18n();
const api = useApi();
const { workspace, manifest, schema, error } = await useWorkspace(ws);
const { data: entries, error: listError } = await useAsyncData(`entries:${ws}`, () =>
  schema.value?.manifest ? api.get<EntrySummary[]>(`/workspaces/${ws}/entries`) : Promise.resolve([] as EntrySummary[]),
);

const typeFilter = ref("");
const localeFilter = ref("");
const shown = computed(() =>
  (entries.value ?? [])
    .filter((e) => !typeFilter.value || e.contentType === typeFilter.value)
    .filter((e) => !localeFilter.value || e.locale === localeFilter.value)
    .sort((a, b) => b.updatedAt.localeCompare(a.updatedAt)),
);
const contentType = (name: string) => manifest.value?.contentTypes.find((c) => c.name === name);

// 新しい記事
const createOpen = ref(false);
const newType = ref("");
const newLocale = ref("");
const creating = ref(false);
const createError = ref<unknown>(null);
function openCreate() {
  newType.value = manifest.value?.contentTypes[0]?.name ?? "";
  newLocale.value = manifest.value?.defaultLocale ?? "";
  createError.value = null;
  createOpen.value = true;
}
async function create() {
  creating.value = true;
  createError.value = null;
  try {
    const draft = await api.post<Draft>(`/workspaces/${ws}/entries`, { contentType: newType.value, locale: newLocale.value });
    await navigateTo(`/w/${ws}/entries/${draft.id}`);
  } catch (e) {
    createError.value = e;
  } finally {
    creating.value = false;
  }
}
</script>

<template>
  <div class="grid gap-6">
    <PageHeader :title="t('nav.entries')" :description="workspace?.name">
      <template v-if="manifest && canEdit(workspace) && schema?.state === 'ok'" #actions>
        <Button @click="openCreate">
          <Plus class="size-4" />{{ t("entries.new") }}
        </Button>
      </template>
    </PageHeader>
    <FormError :error="error ?? listError" />
    <p v-if="schema && !manifest" class="text-muted-foreground">{{ t("entries.noSchema") }}</p>
    <p v-if="schema?.state === 'pending'" class="rounded-md border border-amber-500/50 bg-amber-500/10 p-3 text-sm" role="status">
      {{ t("entries.migrationPending") }}
    </p>

    <template v-if="manifest">
      <div class="flex flex-wrap gap-3 text-sm">
        <label class="flex items-center gap-2">
          {{ t("entries.contentType") }}
          <select v-model="typeFilter" class="h-8 rounded-md border bg-background px-2">
            <option value="">{{ t("entries.all") }}</option>
            <option v-for="c in manifest.contentTypes" :key="c.name" :value="c.name">{{ c.name }}</option>
          </select>
        </label>
        <label class="flex items-center gap-2">
          {{ t("entries.locale") }}
          <select v-model="localeFilter" class="h-8 rounded-md border bg-background px-2">
            <option value="">{{ t("entries.all") }}</option>
            <option v-for="l in manifest.locales" :key="l" :value="l">{{ l }}</option>
          </select>
        </label>
      </div>

      <p v-if="shown.length === 0" class="text-muted-foreground">{{ t("entries.empty") }}</p>
      <div v-else class="overflow-x-auto rounded-md border">
        <table class="w-full text-sm">
          <thead class="bg-muted/50 text-left">
            <tr>
              <th class="px-3 py-2 font-medium">{{ t("entries.title") }}</th>
              <th class="px-3 py-2 font-medium">{{ t("entries.contentType") }}</th>
              <th class="px-3 py-2 font-medium">{{ t("entries.locale") }}</th>
              <th class="px-3 py-2 font-medium">{{ t("entries.status") }}</th>
              <th class="px-3 py-2 font-medium">{{ t("entries.updatedAt") }}</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="e in shown" :key="e.id" class="border-t hover:bg-muted/30">
              <td class="px-3 py-2">
                <NuxtLink :to="`/w/${ws}/entries/${e.id}`" class="font-medium hover:underline">
                  {{ entryTitle(contentType(e.contentType), e.fields) ?? t("entries.untitled") }}
                </NuxtLink>
              </td>
              <td class="px-3 py-2">{{ e.contentType }}</td>
              <td class="px-3 py-2">{{ e.locale }}</td>
              <td class="px-3 py-2">
                <Badge :variant="e.published ? 'default' : 'secondary'">{{ e.published ? t("entries.published") : t("entries.draft") }}</Badge>
              </td>
              <td class="px-3 py-2 text-muted-foreground">{{ formatDateTime(e.updatedAt, locale) }}</td>
            </tr>
          </tbody>
        </table>
      </div>
    </template>

    <Dialog v-model:open="createOpen">
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{{ t("entries.new") }}</DialogTitle>
          <DialogDescription>{{ t("entries.newDescription") }}</DialogDescription>
        </DialogHeader>
        <form class="grid gap-4" @submit.prevent="create">
          <div class="grid gap-2">
            <Label for="new-type">{{ t("entries.contentType") }}</Label>
            <select id="new-type" v-model="newType" class="h-9 rounded-md border bg-background px-3">
              <option v-for="c in manifest?.contentTypes ?? []" :key="c.name" :value="c.name">{{ c.name }}</option>
            </select>
          </div>
          <div class="grid gap-2">
            <Label for="new-locale">{{ t("entries.locale") }}</Label>
            <select id="new-locale" v-model="newLocale" class="h-9 rounded-md border bg-background px-3">
              <option v-for="l in manifest?.locales ?? []" :key="l" :value="l">{{ l }}</option>
            </select>
          </div>
          <FormError :error="createError" />
          <DialogFooter>
            <Button type="button" variant="outline" @click="createOpen = false">{{ t("common.cancel") }}</Button>
            <Button type="submit" :disabled="creating || !newType">{{ t("entries.create") }}</Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  </div>
</template>
