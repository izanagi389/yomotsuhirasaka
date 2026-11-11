<script setup lang="ts">
// 入り口: 所属しているワークスペースの一覧（サーバー管理者はすべて）。選ぶと記事の一覧へ。
// サーバー管理者は、ここでワークスペースを作る（テンプレートの名前を指定。再認証が要る）。
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

const createOpen = ref(false);
const name = ref("");
const template = ref("");
const busy = ref(false);
const createError = ref<unknown>(null);
async function create() {
  busy.value = true;
  createError.value = null;
  try {
    const w = await api.post<{ id: string }>("/workspaces", { name: name.value.trim(), template: template.value.trim() || undefined });
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
            <Input id="ws-template" v-model="template" autocomplete="off" placeholder="default" aria-describedby="ws-template-help" />
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
