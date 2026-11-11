<script setup lang="ts">
// テンプレートの切り替え（02-schema-and-delivery-api.md「テンプレートの切り替え」、INV-SCH-08・09）。
// サーバー管理者と、重い権限「テンプレートの切り替え」を付与されたワークスペースの管理者が使う（判断はサーバーが行う）。
// 1. 切り替え先（デプロイ済みのテンプレート）を選び、試行で対応づけの案を見る（既定は名前と型が同じもの）
// 2. コンテンツタイプとフィールドの対応づけを直す（直すたびに試行し直す）
// 3. 消えるデータがあれば、JSON で書き出してから、明示的に許して切り替える（書き出しと切り替えは再認証が要る）
import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";

interface TemplateOption {
  name: string;
  version: number;
}
interface FieldPlan {
  from: string;
  to: string | null;
  candidates: string[];
}
interface TypePlan {
  from: string;
  to: string | null;
  fields: FieldPlan[];
}
interface Plan {
  contentTypes: TypePlan[];
  newContentTypes: string[];
  added: string[];
  destructive: boolean;
  problems: string[];
}
interface Outcome {
  dryRun: boolean;
  from: string;
  template: string;
  version: number;
  plan: Plan;
  brokenQueries: { workspaceId: string; queryId: string; problems: string[] }[];
}

const props = defineProps<{ workspaceId: string }>();
const { t } = useI18n();
const api = useApi();
const base = computed(() => `/workspaces/${props.workspaceId}`);
const { data: options, error, refresh } = useAsyncData(`template-options:${props.workspaceId}`, () =>
  api.get<TemplateOption[]>(`${base.value}/template-options`),
);

const target = ref("");
const contentTypes = ref<Record<string, string | null>>({});
const fields = ref<Record<string, string | null>>({});
const result = ref<Outcome | null>(null);
const checking = ref(false);
const checkError = ref<unknown>(null);
const mapping = () => ({ contentTypes: contentTypes.value, fields: fields.value });

async function check() {
  if (!target.value) return;
  checking.value = true;
  checkError.value = null;
  try {
    result.value = await api.post<Outcome>(`${base.value}/template`, { template: target.value, mapping: mapping(), dryRun: true });
  } catch (e) {
    result.value = null;
    checkError.value = e;
  } finally {
    checking.value = false;
  }
}
function selectTarget() {
  contentTypes.value = {};
  fields.value = {};
  allowDestructive.value = false;
  allowBrokenQueries.value = false;
  done.value = null;
  return check();
}
function setType(from: string, to: string) {
  contentTypes.value = { ...contentTypes.value, [from]: to === "" ? null : to };
  // コンテンツタイプを変えたら、そのフィールドの対応づけは既定に戻す
  fields.value = Object.fromEntries(Object.entries(fields.value).filter(([k]) => !k.startsWith(`${from}.`)));
  return check();
}
function setField(ct: string, from: string, to: string) {
  fields.value = { ...fields.value, [`${ct}.${from}`]: to === "" ? null : to };
  return check();
}

/** 消えるもの（対応づけのないコンテンツタイプとフィールド）。 */
const lost = computed(() =>
  (result.value?.plan.contentTypes ?? []).flatMap((c) =>
    c.to === null ? [t("templateSwitch.lostType", { name: c.from })] : c.fields.filter((f) => f.to === null).map((f) => `${c.from}.${f.from}`),
  ),
);

// 消えるデータの書き出し（記事の本文を含む。再認証が要る）
const exporting = ref(false);
const exportError = ref<unknown>(null);
const exported = ref(false);
async function exportLost() {
  exporting.value = true;
  exportError.value = null;
  try {
    const data = await api.post<unknown>(`${base.value}/template/export`, { template: target.value, mapping: mapping() });
    const url = URL.createObjectURL(new Blob([JSON.stringify(data, null, 2)], { type: "application/json" }));
    const a = document.createElement("a");
    a.href = url;
    a.download = `template-switch-${props.workspaceId}-${target.value}.json`;
    a.click();
    URL.revokeObjectURL(url);
    exported.value = true;
  } catch (e) {
    exportError.value = e;
  } finally {
    exporting.value = false;
  }
}

const allowDestructive = ref(false);
const allowBrokenQueries = ref(false);
const blocked = computed(() => {
  const r = result.value;
  if (!r || checking.value) return true;
  return (
    r.plan.problems.length > 0 || (r.plan.destructive && !allowDestructive.value) || (r.brokenQueries.length > 0 && !allowBrokenQueries.value)
  );
});
const confirming = ref(false);
const busy = ref(false);
const switchError = ref<unknown>(null);
const done = ref<Outcome | null>(null);
async function run() {
  busy.value = true;
  switchError.value = null;
  try {
    done.value = await api.post<Outcome>(`${base.value}/template`, {
      template: target.value,
      mapping: mapping(),
      allowDestructive: allowDestructive.value,
      allowBrokenQueries: allowBrokenQueries.value,
    });
    confirming.value = false;
    result.value = null;
    target.value = "";
    await refresh();
    await refreshNuxtData(`workspace:${props.workspaceId}`);
  } catch (e) {
    switchError.value = e;
  } finally {
    busy.value = false;
  }
}
</script>

<template>
  <section class="grid gap-4 text-sm" data-testid="template-switch">
    <h2 class="text-lg font-semibold">{{ t("templateSwitch.title") }}</h2>
    <p class="text-muted-foreground">{{ t("templateSwitch.help") }}</p>
    <FormError :error="error ?? checkError" />
    <section v-if="done" class="rounded-md border p-3" role="status">
      {{ t("templateSwitch.done", { from: done.from, template: done.template, version: done.version }) }}
    </section>
    <p v-if="options && options.length === 0" class="text-muted-foreground">{{ t("templateSwitch.noOptions") }}</p>
    <div v-else class="grid max-w-md gap-2">
      <Label for="switch-target">{{ t("templateSwitch.target") }}</Label>
      <select id="switch-target" v-model="target" class="h-9 rounded-md border bg-background px-2" :disabled="checking || busy" @change="selectTarget">
        <option value="" disabled>{{ t("templateSwitch.choose") }}</option>
        <option v-for="o in options ?? []" :key="o.name" :value="o.name">{{ t("workspaces.templateVersion", { name: o.name, version: o.version }) }}</option>
      </select>
    </div>

    <template v-if="result">
      <div class="overflow-x-auto rounded-md border">
        <table class="w-full">
          <thead class="bg-muted/50 text-left">
            <tr>
              <th class="px-3 py-2 font-medium">{{ t("templateSwitch.from") }}</th>
              <th class="px-3 py-2 font-medium">{{ t("templateSwitch.to") }}</th>
            </tr>
          </thead>
          <tbody>
            <template v-for="c in result.plan.contentTypes" :key="c.from">
              <tr class="border-t bg-muted/20">
                <td class="px-3 py-2 font-medium">{{ c.from }}</td>
                <td class="px-3 py-2">
                  <select
                    class="h-8 rounded-md border bg-background px-2"
                    :value="c.to ?? ''"
                    :disabled="checking"
                    :aria-label="t('templateSwitch.typeOf', { name: c.from })"
                    @change="setType(c.from, ($event.target as HTMLSelectElement).value)"
                  >
                    <option value="">{{ t("templateSwitch.remove") }}</option>
                    <option v-for="n in result.plan.newContentTypes" :key="n" :value="n">{{ n }}</option>
                  </select>
                </td>
              </tr>
              <tr v-for="f in c.to === null ? [] : c.fields" :key="`${c.from}.${f.from}`" class="border-t">
                <td class="px-3 py-2 pl-8">{{ f.from }}</td>
                <td class="px-3 py-2">
                  <select
                    class="h-8 rounded-md border bg-background px-2"
                    :value="f.to ?? ''"
                    :disabled="checking"
                    :aria-label="t('templateSwitch.fieldOf', { name: `${c.from}.${f.from}` })"
                    @change="setField(c.from, f.from, ($event.target as HTMLSelectElement).value)"
                  >
                    <option value="">{{ t("templateSwitch.remove") }}</option>
                    <option v-for="n in f.candidates" :key="n" :value="n">{{ n }}</option>
                  </select>
                </td>
              </tr>
            </template>
          </tbody>
        </table>
      </div>
      <p v-if="result.plan.added.length > 0" class="text-muted-foreground">{{ t("templateSwitch.added", { names: result.plan.added.join(", ") }) }}</p>
      <div v-if="result.plan.problems.length > 0">
        <p class="text-destructive">{{ t("templateSwitch.problems") }}</p>
        <ul class="list-disc pl-5">
          <li v-for="p in result.plan.problems" :key="p">{{ p }}</li>
        </ul>
      </div>
      <div v-if="result.plan.destructive" class="grid gap-2 rounded-md border border-destructive/50 p-3">
        <p class="text-destructive">{{ t("templateSwitch.destructive") }}</p>
        <ul class="list-disc pl-5">
          <li v-for="l in lost" :key="l">{{ l }}</li>
        </ul>
        <p class="text-muted-foreground">{{ t("templateSwitch.pastVersions") }}</p>
        <div>
          <Button variant="outline" size="sm" :disabled="exporting || result.plan.problems.length > 0" data-testid="switch-export" @click="exportLost">
            {{ t("templateSwitch.export") }}
          </Button>
          <span v-if="exported" class="ml-2 text-muted-foreground" role="status">{{ t("templateSwitch.exported") }}</span>
        </div>
        <FormError :error="exportError" />
        <label class="flex items-center gap-2"><input v-model="allowDestructive" type="checkbox" />{{ t("templateSwitch.allowDestructive") }}</label>
      </div>
      <div v-if="result.brokenQueries.length > 0" class="grid gap-1">
        <p class="text-destructive">{{ t("templateSwitch.brokenQueries") }}</p>
        <ul class="list-disc pl-5">
          <li v-for="q in result.brokenQueries" :key="q.queryId">{{ q.queryId }}</li>
        </ul>
        <label class="flex items-center gap-2"><input v-model="allowBrokenQueries" type="checkbox" />{{ t("templateSwitch.allowBrokenQueries") }}</label>
      </div>
      <p class="text-muted-foreground">{{ t("templateSwitch.after") }}</p>
      <div>
        <Button :disabled="blocked" data-testid="switch-run" @click="confirming = true">{{ t("templateSwitch.run") }}</Button>
      </div>
    </template>

    <ConfirmDialog
      v-model:open="confirming"
      :title="t('templateSwitch.confirmTitle', { name: target })"
      :description="t(result?.plan.destructive ? 'templateSwitch.confirmDestructive' : 'templateSwitch.confirmDescription')"
      :confirm-label="t('templateSwitch.run')"
      :destructive="result?.plan.destructive ?? false"
      :busy="busy"
      :error="switchError"
      @confirm="run"
    />
  </section>
</template>
