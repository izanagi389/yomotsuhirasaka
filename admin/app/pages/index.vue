<script setup lang="ts">
// 入り口: 所属しているワークスペースの一覧（サーバー管理者はすべて）。選ぶと記事の一覧へ。
import { Card, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";

interface Workspace {
  id: string;
  name: string;
  template: string;
  role: string | null;
  schemaState: "none" | "ok" | "pending";
}

const { t } = useI18n();
const api = useApi();
const { data: workspaces, error } = await useAsyncData("workspaces", () => api.get<Workspace[]>("/workspaces"));
</script>

<template>
  <div class="grid gap-6">
    <h1 class="text-2xl font-semibold">{{ t("workspaces.title") }}</h1>
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
  </div>
</template>
