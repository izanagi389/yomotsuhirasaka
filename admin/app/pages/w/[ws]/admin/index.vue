<script setup lang="ts">
// 管理の入り口。前のタブの形（`?tab=settings` など）で来たときも、対応する画面へ移す。
const route = useRoute();
const ws = route.params.ws as string;
const LEGACY: Record<string, string> = {
  members: "members",
  invitations: "invitations",
  apiKeys: "api-keys",
  webhooks: "webhooks",
  hookSecrets: "hook-secrets",
  correctionTemplates: "correction-templates",
  audit: "audit",
  settings: "settings",
};
await navigateTo(`/w/${ws}/admin/${LEGACY[String(route.query.tab)] ?? "members"}`, { replace: true });
</script>

<template>
  <div />
</template>
