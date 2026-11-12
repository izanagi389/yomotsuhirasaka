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

interface Workspace {
  id: string;
  name: string;
  template: string;
  role: string | null;
  schemaState: "none" | "ok" | "pending";
}

const { t } = useI18n();
const api = useApi();
const { me } = useAuth();
const { data: workspaces, error } = await useAsyncData("workspaces", () => api.get<Workspace[]>("/workspaces"));

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
    <div class="flex flex-wrap items-center gap-3">
      <h1 class="text-2xl font-semibold">{{ t("workspaces.title") }}</h1>
      <Button v-if="me?.isServerAdmin" class="ml-auto" @click="createOpen = true"><Plus class="size-4" />{{ t("workspaces.new") }}</Button>
    </div>
    <FormError :error="error" />
    <p v-if="workspaces && workspaces.length === 0" class="text-muted-foreground">{{ t("workspaces.empty") }}</p>
    <div class="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
      <NuxtLink v-for="w in workspaces ?? []" :key="w.id" :to="`/w/${w.id}`" class="rounded-xl focus-visible:outline-2 focus-visible:outline-ring">
        <Card class="h-full transition-colors hover:bg-muted/40">
          <CardHeader>
            <CardTitle>{{ w.name }}</CardTitle>
            <CardDescription>
              {{ w.role ? t(`roles.${w.role}`) : t("roles.serverAdmin") }} ·
              {{ t(`workspaces.schema.${w.schemaState}`) }}
            </CardDescription>
          </CardHeader>
        </Card>
      </NuxtLink>
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
