<script setup lang="ts">
// 入り口: 所属しているワークスペースの一覧（サーバー管理者はすべて）。選ぶと記事の一覧へ。
// サーバー管理者は、ここでワークスペースを作る（テンプレートを選ぶか名前を入れる。再認証が要る）。
// テンプレートの選択肢は、デプロイ済みのものと、まだデプロイしていない同梱のもの（選べない形で出す）。
// まだデプロイしていないテンプレートの名前は「その他」で入れる（後からデプロイすると適用される）。
import { Plus } from "@lucide/vue";

import { Button } from "@/components/ui/button";
import { Card, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";

const { t } = useI18n();
const api = useApi();
const { me } = useAuth();
const { data: workspaces, error } = await useWorkspaces();

interface TemplateOption {
  name: string;
  version: number | null;
  bundled: boolean;
}
const OTHER = "__other__";
const createOpen = ref(false);
const name = ref("");
const templates = ref<TemplateOption[]>([]);
const choice = ref("");
const otherName = ref("");
const template = computed(() => (choice.value === OTHER ? otherName.value.trim() : choice.value));
const busy = ref(false);
const createError = ref<unknown>(null);
watch(createOpen, async (open) => {
  if (!open || !me.value?.isServerAdmin) return;
  try {
    templates.value = await api.get<TemplateOption[]>("/templates");
  } catch (e) {
    createError.value = e;
  }
  if (!choice.value) choice.value = templates.value.find((o) => o.version !== null)?.name ?? OTHER;
});
async function create() {
  busy.value = true;
  createError.value = null;
  try {
    const w = await api.post<{ id: string }>("/workspaces", { name: name.value.trim(), template: template.value || undefined });
    await navigateTo(`/w/${w.id}`);
  } catch (e) {
    createError.value = e;
  } finally {
    busy.value = false;
  }
}
</script>

<template>
  <div class="grid gap-6">
    <PageHeader :title="t('workspaces.title')">
      <template v-if="me?.isServerAdmin" #actions>
        <Button @click="createOpen = true"><Plus class="size-4" />{{ t("workspaces.new") }}</Button>
      </template>
    </PageHeader>
    <FormError :error="error" />
    <EmptyState v-if="workspaces && workspaces.length === 0" :title="t('workspaces.empty')" />
    <div class="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
      <Card v-for="w in workspaces ?? []" :key="w.id" class="relative h-full transition-colors hover:bg-muted/40" data-testid="workspace-card">
        <CardHeader class="flex flex-row items-start gap-3">
          <Kamon :id="w.id" :crest="w.crest" :size="40" />
          <div class="grid min-w-0 flex-1 gap-1">
            <CardTitle class="truncate text-base">
              <NuxtLink :to="`/w/${w.id}`" class="rounded-sm outline-none after:absolute after:inset-0 after:rounded-xl focus-visible:ring-2 focus-visible:ring-ring">{{ w.name }}</NuxtLink>
            </CardTitle>
            <CardDescription class="text-xs">
              {{ t("workspaces.templateLabel", { name: w.template }) }} ·
              {{ w.role ? t(`roles.${w.role}`) : t("roles.serverAdmin") }}
            </CardDescription>
            <span class="text-xs" :class="w.schemaState === 'ok' ? 'text-muted-foreground' : 'text-amber-700 dark:text-amber-400'">{{ t(`workspaces.schema.${w.schemaState}`) }}</span>
          </div>
          <div class="relative z-10 -mt-1 -mr-2">
            <WorkspaceActions :workspace="w" />
          </div>
        </CardHeader>
      </Card>
    </div>

    <Dialog v-model:open="createOpen">
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{{ t("workspaces.new") }}</DialogTitle>
          <DialogDescription>{{ t("workspaces.newDescription") }}</DialogDescription>
        </DialogHeader>
        <form class="grid gap-4" @submit.prevent="create">
          <div class="grid gap-2">
            <Label for="ws-name">{{ t("workspaces.name") }}</Label>
            <Input id="ws-name" v-model="name" required autocomplete="off" />
          </div>
          <div class="grid gap-2">
            <Label for="ws-template">{{ t("workspaces.template") }}</Label>
            <select id="ws-template" v-model="choice" class="h-9 rounded-md border bg-background px-2" aria-describedby="ws-template-help" data-testid="ws-template">
              <option v-for="o in templates" :key="o.name" :value="o.name" :disabled="o.version === null">
                {{ o.version === null ? t("workspaces.templateNotDeployed", { name: o.name }) : t("workspaces.templateVersion", { name: o.name, version: o.version }) }}
              </option>
              <option :value="OTHER">{{ t("workspaces.templateOther") }}</option>
            </select>
            <Input v-if="choice === OTHER" v-model="otherName" autocomplete="off" placeholder="default" :aria-label="t('workspaces.templateOtherName')" />
            <p id="ws-template-help" class="text-xs text-muted-foreground">{{ t("workspaces.templateHelp") }}</p>
          </div>
          <FormError :error="createError" />
          <DialogFooter>
            <Button type="button" variant="outline" @click="createOpen = false">{{ t("common.cancel") }}</Button>
            <Button type="submit" :disabled="busy || !name.trim()">{{ t("entries.create") }}</Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  </div>
</template>
