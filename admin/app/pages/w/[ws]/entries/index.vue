<script setup lang="ts">
// ワークスペースの記事の一覧（/w/{ws}/entries）。記事の作成もここから行う。ワークスペースの入り口は「やること」（/w/{ws}）。
import { Plus, Search } from "@lucide/vue";

import { Input } from "@/components/ui/input";
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
const statusFilter = ref<"all" | "published" | "draft">("all");
const query = ref("");
// 状態の絞り込みは選択肢の組（radiogroup）なので、Tab では1つにだけ止まり、矢印キーで選び直す（WAI-ARIA の作法）
const STATUSES = ["all", "published", "draft"] as const;
function moveStatus(e: KeyboardEvent) {
  const step = e.key === "ArrowRight" || e.key === "ArrowDown" ? 1 : e.key === "ArrowLeft" || e.key === "ArrowUp" ? -1 : 0;
  if (step === 0) return;
  e.preventDefault();
  const group = e.currentTarget as HTMLElement;
  const next = STATUSES[(STATUSES.indexOf(statusFilter.value) + step + STATUSES.length) % STATUSES.length]!;
  statusFilter.value = next;
  void nextTick(() => group.querySelector<HTMLElement>(`[data-status="${next}"]`)?.focus());
}
const contentType = (name: string) => manifest.value?.contentTypes.find((c) => c.name === name);
const titleOf = (e: EntrySummary) => entryTitle(contentType(e.contentType), e.fields);
const shown = computed(() => {
  const q = query.value.trim().toLowerCase();
  return (entries.value ?? [])
    .filter((e) => !typeFilter.value || e.contentType === typeFilter.value)
    .filter((e) => !localeFilter.value || e.locale === localeFilter.value)
    .filter((e) => statusFilter.value === "all" || (statusFilter.value === "published") === e.published)
    .filter((e) => !q || (titleOf(e) ?? "").toLowerCase().includes(q))
    .sort((a, b) => b.updatedAt.localeCompare(a.updatedAt));
});
const canCreate = computed(() => !!manifest.value && canEdit(workspace.value) && schema.value?.state === "ok");
const counts = computed(() => {
  const all = entries.value ?? [];
  return { all: all.length, published: all.filter((e) => e.published).length, draft: all.filter((e) => !e.published).length };
});

// 新しい記事
const createOpen = ref(false);
const newType = ref("");
const newLocale = ref("");
const creating = ref(false);
const createError = ref<unknown>(null);
// 選ぶものが1つしかない欄は聞かない。種類も言語も1つなら、画面を挟まずにすぐ作る
const manyTypes = computed(() => (manifest.value?.contentTypes.length ?? 0) > 1);
const manyLocales = computed(() => (manifest.value?.locales.length ?? 0) > 1);
function openCreate() {
  newType.value = manifest.value?.contentTypes[0]?.name ?? "";
  newLocale.value = manifest.value?.defaultLocale ?? "";
  createError.value = null;
  if (!manyTypes.value && !manyLocales.value) {
    void create();
    return;
  }
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
    <PageHeader :title="t('nav.entries')" :description="manifest ? t('entries.count', { n: counts.all }) : workspace?.name">
      <template v-if="canCreate && counts.all > 0" #actions>
        <Button :disabled="creating" @click="openCreate">
          <Plus class="size-4" />{{ t("entries.new") }}
        </Button>
      </template>
    </PageHeader>
    <FormError :error="error ?? listError ?? (createOpen ? null : createError)" />
    <p v-if="schema && !manifest" class="text-muted-foreground">{{ t("entries.noSchema") }}</p>
    <p v-if="schema?.state === 'pending'" class="rounded-md border border-warning/50 bg-warning/10 p-3 text-sm" role="status">
      {{ t("entries.migrationPending") }}
    </p>

    <template v-if="manifest">
      <EmptyState v-if="counts.all === 0" :title="t('entries.empty')" :description="canCreate ? t('entries.emptyHelp') : undefined">
        <Button v-if="canCreate" :disabled="creating" @click="openCreate">
          <Plus class="size-4" />{{ t("entries.new") }}
        </Button>
      </EmptyState>
      <div v-else class="flex flex-wrap items-center gap-2.5 text-sm">
        <div role="radiogroup" :aria-label="t('entries.status')" class="flex rounded-[9px] bg-muted p-[3px]" @keydown="moveStatus">
          <button
            v-for="f in STATUSES"
            :key="f"
            type="button"
            role="radio"
            :aria-checked="statusFilter === f"
            :tabindex="statusFilter === f ? 0 : -1"
            :data-status="f"
            class="h-8 rounded-[7px] px-3 text-[13px]"
            :class="statusFilter === f ? 'bg-card font-medium shadow-sm' : 'text-muted-foreground hover:text-foreground'"
            @click="statusFilter = f"
          >
            {{ t(`entries.filter.${f}`) }} <span class="text-muted-foreground">{{ counts[f] }}</span>
          </button>
        </div>
        <label class="relative min-w-[200px] max-w-xs flex-1">
          <span class="sr-only">{{ t("entries.search") }}</span>
          <Search class="pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted-foreground" aria-hidden="true" />
          <Input v-model="query" type="search" :placeholder="t('entries.searchPlaceholder')" class="h-9 pl-8" />
        </label>
        <select v-if="manifest.contentTypes.length > 1" v-model="typeFilter" class="h-9 px-2" :aria-label="t('entries.contentType')">
          <option value="">{{ t("entries.contentType") }}: {{ t("entries.all") }}</option>
          <option v-for="c in manifest.contentTypes" :key="c.name" :value="c.name">{{ typeLabel(c, locale) }}</option>
        </select>
        <select v-if="manifest.locales.length > 1" v-model="localeFilter" class="h-9 px-2" :aria-label="t('entries.locale')">
          <option value="">{{ t("entries.locale") }}: {{ t("entries.all") }}</option>
          <option v-for="l in manifest.locales" :key="l" :value="l">{{ localeName(l, locale) }}</option>
        </select>
      </div>

      <EmptyState v-if="counts.all > 0 && shown.length === 0" :title="t('entries.noMatch')" />
      <div v-else-if="counts.all > 0" class="overflow-x-auto rounded-xl border bg-card">
        <table class="w-full min-w-[640px] text-sm">
          <thead class="bg-muted/50 text-left text-xs text-muted-foreground">
            <tr>
              <th class="px-5 py-2.5 font-medium">{{ t("entries.title") }}</th>
              <th class="px-3 py-2.5 font-medium">{{ t("entries.status") }}</th>
              <th v-if="manifest.contentTypes.length > 1" class="px-3 py-2.5 font-medium">{{ t("entries.contentType") }}</th>
              <th v-if="manifest.locales.length > 1" class="px-3 py-2.5 font-medium">{{ t("entries.locale") }}</th>
              <th class="px-3 py-2.5 font-medium">{{ t("entries.updatedAt") }}</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="e in shown" :key="e.id" class="relative border-t hover:bg-muted/40">
              <td class="px-5 py-3">
                <NuxtLink :to="`/w/${ws}/entries/${e.id}`" class="font-medium after:absolute after:inset-0 hover:underline focus-visible:outline-2 focus-visible:outline-ring">
                  {{ titleOf(e) ?? t("entries.untitled") }}
                </NuxtLink>
              </td>
              <td class="px-3 py-3">
                <span class="inline-flex items-center gap-1.5 text-xs font-medium whitespace-nowrap" :class="e.published ? 'text-success-text' : 'text-muted-foreground'">
                  <svg aria-hidden="true" width="10" height="10" viewBox="0 0 12 12"><path d="M6 0.8L11.2 6L6 11.2L0.8 6Z" fill="currentColor" /></svg>
                  {{ e.published ? t("entries.published") : t("entries.draft") }}
                </span>
              </td>
              <td v-if="manifest.contentTypes.length > 1" class="px-3 py-3 text-muted-foreground">{{ typeLabel(e.contentType, locale, manifest) }}</td>
              <td v-if="manifest.locales.length > 1" class="px-3 py-3 whitespace-nowrap text-muted-foreground">{{ localeName(e.locale, locale) }}</td>
              <td class="px-3 py-3 whitespace-nowrap text-muted-foreground">{{ formatDateTime(e.updatedAt, locale) }}</td>
            </tr>
          </tbody>
        </table>
      </div>
    </template>

    <Dialog v-model:open="createOpen">
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{{ t("entries.new") }}</DialogTitle>
          <DialogDescription>{{ manyTypes ? t("entries.newDescription") : t("entries.newDescriptionLocale") }}</DialogDescription>
        </DialogHeader>
        <form class="grid gap-4" @submit.prevent="create">
          <div v-if="manyTypes" class="grid gap-2">
            <Label for="new-type">{{ t("entries.contentType") }}</Label>
            <select id="new-type" v-model="newType" class="h-9 rounded-md border bg-background px-3">
              <option v-for="c in manifest?.contentTypes ?? []" :key="c.name" :value="c.name">{{ typeLabel(c, locale) }}</option>
            </select>
          </div>
          <div v-if="manyLocales" class="grid gap-2">
            <Label for="new-locale">{{ t("entries.locale") }}</Label>
            <select id="new-locale" v-model="newLocale" class="h-9 rounded-md border bg-background px-3">
              <option v-for="l in manifest?.locales ?? []" :key="l" :value="l">{{ localeName(l, locale) }}</option>
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
