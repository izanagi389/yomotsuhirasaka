<script setup lang="ts">
// 自分のアカウントの画面の切り替え。多要素認証の設定が要る間は、セキュリティの画面だけを使える（Q197）。
const { t } = useI18n();
const route = useRoute();
const { me } = useAuth();
const items = computed(() => [
  { to: "/account/security", label: t("account.nav.security") },
  ...(me.value?.mfaSetupRequired
    ? []
    : [
        { to: "/account/profile", label: t("account.nav.profile") },
        { to: "/account/sessions", label: t("account.nav.sessions") },
        { to: "/account/tokens", label: t("account.nav.tokens") },
      ]),
]);
</script>

<template>
  <!-- 見た目は、ほかの画面のタブ（components/ui/tabs）とそろえる -->
  <nav class="inline-flex w-fit max-w-full flex-wrap items-center gap-0.5 rounded-lg bg-muted p-0.75 text-sm" :aria-label="t('account.nav.label')">
    <NuxtLink
      v-for="i in items"
      :key="i.to"
      :to="i.to"
      class="inline-flex h-8 items-center rounded-md border border-transparent px-3 font-medium whitespace-nowrap outline-none focus-visible:ring-3 focus-visible:ring-ring/50"
      :class="route.path === i.to ? 'bg-background text-foreground shadow-sm dark:border-input dark:bg-input/30' : 'text-muted-foreground hover:text-foreground'"
      :aria-current="route.path === i.to ? 'page' : undefined"
    >
      {{ i.label }}
    </NuxtLink>
  </nav>
</template>
