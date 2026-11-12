<script setup lang="ts">
// ワークスペースの紋を選ぶ（管理者・サーバー管理者。変更は再認証が要り、監査ログに残る）。
// 形は12種類、色は6色。選ばなければ（「自動」）IDから決める。値の検査はサーバーが行う。
import { Button } from "@/components/ui/button";
import { autoCrest, KAMON_COLORS, KAMON_MOTIFS, type Crest } from "~/utils/kamon";
import type { WorkspaceInfo } from "~/utils/content";

const props = defineProps<{ workspaceId: string }>();
const { t } = useI18n();
const api = useApi();
const toast = useToast();
const { data: workspace, refresh } = useAsyncData(`workspace-crest:${props.workspaceId}`, () =>
  api.get<WorkspaceInfo>(`/workspaces/${props.workspaceId}`),
);

const COLOR_KEYS = Object.keys(KAMON_COLORS) as (keyof typeof KAMON_COLORS)[];
const fallback = computed(() => autoCrest(props.workspaceId));
const motif = ref("");
const color = ref("");
watch(
  workspace,
  (w) => {
    const c = w?.crest ?? fallback.value;
    motif.value = c.motif;
    color.value = c.color;
  },
  { immediate: true },
);
const chosen = computed<Crest>(() => ({ motif: motif.value, color: color.value }));
const isAuto = computed(() => !workspace.value?.crest);
const changed = computed(() => {
  const c = workspace.value?.crest;
  return c ? c.motif !== motif.value || c.color !== color.value : motif.value !== fallback.value.motif || color.value !== fallback.value.color;
});

const saving = ref(false);
const saveError = ref<unknown>(null);
async function save(crest: Crest | null) {
  saving.value = true;
  saveError.value = null;
  try {
    await api.patch(`/workspaces/${props.workspaceId}/settings`, { crest });
    await refresh();
    await refreshNuxtData([`ws-nav:${props.workspaceId}`, `workspace:${props.workspaceId}`, "workspaces"]);
    toast.show(t("crest.saved"));
  } catch (e) {
    saveError.value = e;
  } finally {
    saving.value = false;
  }
}
</script>

<template>
  <section class="grid gap-4 text-sm" data-testid="crest-panel">
    <div class="grid gap-1">
      <h2 class="text-base font-semibold">{{ t("crest.title") }}</h2>
      <p class="text-muted-foreground">{{ t("crest.help") }}</p>
    </div>
    <div class="flex flex-wrap items-start gap-6">
      <div class="grid justify-items-center gap-2">
        <Kamon :id="workspaceId" :crest="chosen" :size="72" />
        <span class="text-xs text-muted-foreground">{{ isAuto && !changed ? t("crest.auto") : t("crest.chosen") }}</span>
      </div>
      <div class="grid flex-1 gap-4">
        <fieldset class="grid gap-2">
          <legend class="mb-2 text-xs font-medium text-muted-foreground">{{ t("crest.motif") }}</legend>
          <div class="flex flex-wrap gap-2">
            <label
              v-for="m in KAMON_MOTIFS"
              :key="m"
              class="grid cursor-pointer justify-items-center gap-1 rounded-lg border p-2 has-[:checked]:border-primary has-[:checked]:bg-accent has-[:focus-visible]:ring-2 has-[:focus-visible]:ring-ring"
            >
              <input v-model="motif" type="radio" name="crest-motif" :value="m" class="sr-only" />
              <Kamon :id="workspaceId" :crest="{ motif: m, color }" :size="32" />
              <span class="text-[11px]">{{ t(`crest.motifs.${m}`) }}</span>
            </label>
          </div>
        </fieldset>
        <fieldset class="grid gap-2">
          <legend class="mb-2 text-xs font-medium text-muted-foreground">{{ t("crest.color") }}</legend>
          <div class="flex flex-wrap gap-2">
            <label
              v-for="c in COLOR_KEYS"
              :key="c"
              class="flex cursor-pointer items-center gap-2 rounded-lg border px-2.5 py-1.5 has-[:checked]:border-primary has-[:checked]:bg-accent has-[:focus-visible]:ring-2 has-[:focus-visible]:ring-ring"
            >
              <input v-model="color" type="radio" name="crest-color" :value="c" class="sr-only" />
              <span aria-hidden="true" class="size-4 rounded-full border" :style="{ backgroundColor: KAMON_COLORS[c].bg, borderColor: KAMON_COLORS[c].fg }" />
              <span class="text-xs">{{ t(`crest.colors.${c}`) }}</span>
            </label>
          </div>
        </fieldset>
        <FormError :error="saveError" />
        <div class="flex flex-wrap gap-2">
          <Button :disabled="saving || !changed" data-testid="crest-save" @click="save(chosen)">{{ t("crest.save") }}</Button>
          <Button variant="outline" :disabled="saving || isAuto" data-testid="crest-reset" @click="save(null)">{{ t("crest.reset") }}</Button>
        </div>
      </div>
    </div>
  </section>
</template>
