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
        { to: "/account/sessions", label: t("account.nav.sessions") },
        { to: "/account/tokens", label: t("account.nav.tokens") },
      ]),
]);
</script>

<template>
  <nav class="flex gap-4 border-b text-sm" :aria-label="t('account.nav.label')">
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
</template>
