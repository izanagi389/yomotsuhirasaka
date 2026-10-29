<script setup lang="ts">
// 記事の関連記事（08-search-and-related.md「関連記事」、Q60・Q157）。
// - 今返している関連記事（配信と同じ照合をかけた上限 10 件）を見せ、固定・除外を決める材料にする
// - 固定（順番付き、5件まで）と除外（50件まで）は翻訳グループを指し、返すときに同じロケールの記事へ解決する
// - 変えるとすぐに効く（版ではなく記事に持つ）。変えられるのは公開できる人。権限と件数の判断はサーバー
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { type ContentType, type EntrySummary, type RelatedView, entryTitle } from "~/utils/content";

const MAX_PINNED = 5;
const MAX_EXCLUDED = 50;

const props = defineProps<{
  workspaceId: string;
  entryId: string;
  contentType: ContentType;
  locale: string;
  entries: EntrySummary[];
  editable: boolean;
}>();
const { t, locale: uiLocale } = useI18n();
const api = useApi();
const path = computed(() => `/workspaces/${props.workspaceId}/entries/${props.entryId}/related`);
const { data: view, error } = useAsyncData(`related:${props.entryId}`, () => api.get<RelatedView>(path.value));

const ownGroup = computed(() => props.entries.find((e) => e.id === props.entryId)?.translationGroupId);
// 候補: 同じコンテンツタイプの記事を翻訳グループごとにまとめ、この記事と同じロケールの名前（なければ他のロケール）で出す
const groups = computed(() => {
  const map = new Map<string, EntrySummary>();
  for (const e of props.entries.filter((e) => e.contentType === props.contentType.name && e.translationGroupId !== ownGroup.value)) {
    const current = map.get(e.translationGroupId);
    if (!current || (e.locale === props.locale && current.locale !== props.locale)) map.set(e.translationGroupId, e);
  }
  return map;
});
const labelOf = (e: EntrySummary | undefined, fallback: string) => (e ? `${entryTitle(props.contentType, e.fields) ?? t("entries.untitled")} (${e.locale})` : fallback);
const groupLabel = (group: string) => labelOf(groups.value.get(group), t("related.unknownEntry"));
const entryLabel = (id: string) => labelOf(props.entries.find((e) => e.id === id), id);
const unused = computed(() => [...groups.value.keys()].filter((g) => !view.value?.pinned.includes(g) && !view.value?.excluded.includes(g)));

const saving = ref(false);
const saveError = ref<unknown>(null);
async function save(pinned: string[], excluded: string[]) {
  saving.value = true;
  saveError.value = null;
  try {
    view.value = await api.put<RelatedView>(path.value, { pinned, excluded });
  } catch (e) {
    saveError.value = e;
  } finally {
    saving.value = false;
  }
}
const pinned = computed(() => view.value?.pinned ?? []);
const excluded = computed(() => view.value?.excluded ?? []);
function move(i: number, by: -1 | 1) {
  const next = [...pinned.value];
  const [g] = next.splice(i, 1);
  next.splice(i + by, 0, g!);
  void save(next, excluded.value);
}
function addTo(kind: "pinned" | "excluded", event: Event) {
  const select = event.target as HTMLSelectElement;
  const group = select.value;
  select.value = "";
  if (!group) return;
  if (kind === "pinned") void save([...pinned.value, group], excluded.value);
  else void save(pinned.value, [...excluded.value, group]);
}
</script>

<template>
  <div class="grid gap-6 text-sm" data-testid="related-panel">
    <FormError :error="error ?? saveError" />
    <p class="text-muted-foreground">{{ t("related.note") }}</p>
    <p v-if="!editable" class="text-muted-foreground">{{ t("related.needPermission") }}</p>
    <template v-if="view">
      <section class="grid gap-2">
        <h3 class="font-medium">{{ t("related.current") }}</h3>
        <p v-if="view.current.length === 0" class="text-muted-foreground">{{ t("related.none") }}</p>
        <ol v-else class="grid gap-1" data-testid="related-current">
          <li v-for="(c, i) in view.current" :key="c.entryId" class="flex flex-wrap items-center gap-2 rounded-md border px-3 py-2">
            <span class="w-6 text-muted-foreground tabular-nums">{{ i + 1 }}</span>
            <span class="font-medium">{{ entryLabel(c.entryId) }}</span>
            <Badge v-if="c.pinned" variant="secondary">{{ t("related.pinnedBadge") }}</Badge>
            <span v-if="!c.pinned" class="ml-auto text-muted-foreground tabular-nums">{{ c.score.toLocaleString(uiLocale, { maximumFractionDigits: 3 }) }}</span>
          </li>
        </ol>
      </section>

      <section class="grid gap-2">
        <h3 class="font-medium">{{ t("related.pinned", { max: MAX_PINNED }) }}</h3>
        <p v-if="pinned.length === 0" class="text-muted-foreground">{{ t("related.nonePinned") }}</p>
        <ol v-else class="grid gap-1" data-testid="related-pinned">
          <li v-for="(g, i) in pinned" :key="g" class="flex flex-wrap items-center gap-2 rounded-md border px-3 py-2">
            <span class="font-medium">{{ groupLabel(g) }}</span>
            <span v-if="editable" class="ml-auto flex gap-1">
              <Button size="sm" variant="ghost" :disabled="saving || i === 0" :aria-label="t('related.moveUp')" @click="move(i, -1)">↑</Button>
              <Button size="sm" variant="ghost" :disabled="saving || i === pinned.length - 1" :aria-label="t('related.moveDown')" @click="move(i, 1)">↓</Button>
              <Button size="sm" variant="outline" :disabled="saving" @click="save(pinned.filter((x) => x !== g), excluded)">{{ t("related.remove") }}</Button>
            </span>
          </li>
        </ol>
        <select
          v-if="editable && pinned.length < MAX_PINNED"
          class="h-9 w-fit rounded-md border bg-background px-2"
          :disabled="saving"
          :aria-label="t('related.addPinned')"
          data-testid="related-add-pinned"
          @change="addTo('pinned', $event)"
        >
          <option value="">{{ t("related.addPinned") }}</option>
          <option v-for="g in unused" :key="g" :value="g">{{ groupLabel(g) }}</option>
        </select>
      </section>

      <section class="grid gap-2">
        <h3 class="font-medium">{{ t("related.excluded", { max: MAX_EXCLUDED }) }}</h3>
        <p v-if="excluded.length === 0" class="text-muted-foreground">{{ t("related.noneExcluded") }}</p>
        <ul v-else class="grid gap-1" data-testid="related-excluded">
          <li v-for="g in excluded" :key="g" class="flex flex-wrap items-center gap-2 rounded-md border px-3 py-2">
            <span>{{ groupLabel(g) }}</span>
            <Button v-if="editable" size="sm" variant="outline" class="ml-auto" :disabled="saving" @click="save(pinned, excluded.filter((x) => x !== g))">{{ t("related.remove") }}</Button>
          </li>
        </ul>
        <select
          v-if="editable && excluded.length < MAX_EXCLUDED"
          class="h-9 w-fit rounded-md border bg-background px-2"
          :disabled="saving"
          :aria-label="t('related.addExcluded')"
          data-testid="related-add-excluded"
          @change="addTo('excluded', $event)"
        >
          <option value="">{{ t("related.addExcluded") }}</option>
          <option v-for="g in unused" :key="g" :value="g">{{ groupLabel(g) }}</option>
        </select>
      </section>
    </template>
  </div>
</template>
