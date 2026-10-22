<script setup lang="ts">
// 訂正の説明の定型文（06-corrections-and-freshness.md、Q63）。管理者とサーバー管理者が、ロケールごとに編集する。
// - 選べる文（choice）は、訂正として公開するときに選ぶ。選んだ時点の文が説明に写されるので、後から変えても過去の説明は変わらない
// - 説明なしのときの文（fallback）は、説明を書いていない訂正に配信APIが出す文。ロケールに1つ
// 検査はサーバーが行う。
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import type { CorrectionTemplate } from "~/utils/content";

const props = defineProps<{ workspaceId: string; locales: string[] }>();
const { t } = useI18n();
const api = useApi();
const path = computed(() => `/workspaces/${props.workspaceId}/correction-templates`);
const { data: templates, error } = useAsyncData(`correction-templates:${props.workspaceId}`, () => api.get<CorrectionTemplate[]>(path.value));

const list = ref<CorrectionTemplate[]>([]);
watch(templates, (v) => (list.value = (v ?? []).map((x) => ({ ...x }))), { immediate: true });

const choicesOf = (l: string) => list.value.filter((x) => x.locale === l && x.kind === "choice");
const fallbackOf = (l: string) => list.value.find((x) => x.locale === l && x.kind === "fallback");
function setFallback(l: string, text: string) {
  const f = fallbackOf(l);
  if (f) f.text = text;
  else list.value.push({ locale: l, kind: "fallback", text });
}
function add(l: string) {
  list.value.push({ locale: l, kind: "choice", text: "" });
}
function remove(item: CorrectionTemplate) {
  list.value = list.value.filter((x) => x !== item);
}

const saving = ref(false);
const saveError = ref<unknown>(null);
const saved = ref(false);
async function save() {
  saving.value = true;
  saveError.value = null;
  saved.value = false;
  try {
    // 空の「説明なしのときの文」は送らない（その場合、説明のない訂正には説明を出さない）
    const body = list.value.filter((x) => x.kind === "choice" || x.text.trim() !== "");
    templates.value = await api.put<CorrectionTemplate[]>(path.value, body);
    saved.value = true;
  } catch (e) {
    saveError.value = e;
  } finally {
    saving.value = false;
  }
}
</script>

<template>
  <div class="grid gap-6 text-sm">
    <FormError :error="error ?? saveError" />
    <p class="text-muted-foreground">{{ t("correctionTemplates.help") }}</p>
    <section v-for="l in locales" :key="l" class="grid gap-3 rounded-md border p-4" :aria-label="l">
      <h3 class="font-medium">{{ l }}</h3>
      <div class="grid gap-1.5">
        <label :for="`fallback-${l}`" class="text-muted-foreground">{{ t("correctionTemplates.fallback") }}</label>
        <Input :id="`fallback-${l}`" :model-value="fallbackOf(l)?.text ?? ''" maxlength="2000" @update:model-value="(v) => setFallback(l, String(v))" />
      </div>
      <div class="grid gap-1.5">
        <span class="text-muted-foreground">{{ t("correctionTemplates.choices") }}</span>
        <div v-for="(c, i) in choicesOf(l)" :key="c.id ?? `new-${i}`" class="flex gap-2">
          <Input v-model="c.text" maxlength="2000" :aria-label="t('correctionTemplates.choice', { n: i + 1 })" />
          <Button variant="outline" size="sm" @click="remove(c)">{{ t("common.remove") }}</Button>
        </div>
        <Button variant="outline" size="sm" class="w-fit" @click="add(l)">{{ t("correctionTemplates.add") }}</Button>
      </div>
    </section>
    <div class="flex items-center gap-3">
      <Button :disabled="saving" @click="save">{{ t("common.save") }}</Button>
      <span v-if="saved" role="status" class="text-muted-foreground">{{ t("correctionTemplates.saved") }}</span>
    </div>
  </div>
</template>
