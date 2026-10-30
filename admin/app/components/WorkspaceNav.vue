<script setup lang="ts">
// ワークスペースの中の画面の切り替え（やること・記事・メディア・予算・チャットボット・管理）。管理は管理者とサーバー管理者だけ、
// 予算とチャットボットは重い権限 manageLlm を持つ人とサーバー管理者だけに出す（権限の判断はサーバー）。
// 「やること」には、その人が対応できる項目の件数を出す（0件なら出さない）。
const props = defineProps<{ workspaceId: string; name: string | undefined; admin?: boolean; budget?: boolean }>();
const { t } = useI18n();
const route = useRoute();
const { data: todo } = useTodo(props.workspaceId);
const items = computed(() => [
  { to: `/w/${props.workspaceId}`, label: t("nav.todo"), count: todo.value?.length ?? 0 },
  { to: `/w/${props.workspaceId}/entries`, label: t("nav.entries") },
  { to: `/w/${props.workspaceId}/media`, label: t("nav.media") },
  ...(props.budget
    ? [
        { to: `/w/${props.workspaceId}/budget`, label: t("nav.budget") },
        { to: `/w/${props.workspaceId}/chatbot`, label: t("nav.chatbot") },
      ]
    : []),
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
        <span v-if="'count' in i && i.count" class="ml-1 rounded-full bg-primary px-1.5 text-xs text-primary-foreground" data-testid="todo-count">{{ i.count }}</span>
      </NuxtLink>
    </nav>
  </div>
</template>
