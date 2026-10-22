<script setup lang="ts">
// ワークスペースの中の画面の切り替え（記事・要対応・メディア・管理）。管理は管理者とサーバー管理者だけに出す（権限の判断はサーバー）。
const props = defineProps<{ workspaceId: string; name: string | undefined; admin?: boolean }>();
const { t } = useI18n();
const route = useRoute();
const items = computed(() => [
  { to: `/w/${props.workspaceId}`, label: t("nav.entries") },
  { to: `/w/${props.workspaceId}/checks`, label: t("nav.checks") },
  { to: `/w/${props.workspaceId}/media`, label: t("nav.media") },
  ...(props.admin ? [{ to: `/w/${props.workspaceId}/admin`, label: t("nav.admin") }] : []),
]);
</script>

<template>
  <div class="grid gap-2">
    <NuxtLink to="/" class="text-sm text-muted-foreground hover:text-foreground">{{ t("workspaces.title") }}</NuxtLink>
    <h1 class="text-2xl font-semibold">{{ name }}</h1>
    <nav class="flex gap-4 border-b text-sm" :aria-label="name">
      <NuxtLink
        v-for="i in items"
        :key="i.to"
        :to="i.to"
        class="-mb-px border-b-2 px-1 pb-2"
        :class="route.path === i.to ? 'border-primary font-medium' : 'border-transparent text-muted-foreground hover:text-foreground'"
        :aria-current="route.path === i.to ? 'page' : undefined"
      >
        {{ i.label }}
      </NuxtLink>
    </nav>
  </div>
</template>
