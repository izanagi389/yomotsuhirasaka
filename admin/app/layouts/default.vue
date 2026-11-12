<script setup lang="ts">
// 管理画面の骨組み。ワークスペースの中では左にサイドバー（狭い画面では「≡」で開く）、
// それ以外（ワークスペースの一覧、アカウント、サーバーの管理）では上に藍の帯を置く。
import { Menu, X } from "@lucide/vue";

const { me } = useAuth();
const { t } = useI18n();
const route = useRoute();

const ws = computed(() => (typeof route.params.ws === "string" ? route.params.ws : null));
// 記事の編集画面では、本文を広く取るためにサイドバーをアイコンだけにし、余白も画面に任せる
const editor = computed(() => /\/w\/[^/]+\/entries\/[^/]+$/.test(route.path));
const menuOpen = ref(false);
watch(
  () => route.fullPath,
  () => {
    menuOpen.value = false;
  },
);
</script>

<template>
  <div v-if="ws" class="min-h-svh md:flex">
    <aside class="sticky top-0 hidden h-svh shrink-0 md:block" :class="editor ? 'w-16' : 'w-60'">
      <WorkspaceSidebar :key="ws" :workspace-id="ws" :compact="editor" />
    </aside>

    <div class="flex h-14 items-center gap-3 bg-sidebar px-4 text-sidebar-strong md:hidden">
      <button type="button" class="-ml-2 rounded-md p-2 hover:bg-sidebar-active" :aria-label="t('nav.openMenu')" :aria-expanded="menuOpen" @click="menuOpen = true">
        <Menu class="size-5" aria-hidden="true" />
      </button>
      <BrandMark :size="24" />
      <span class="font-mincho tracking-[0.18em]">{{ t("app.name") }}</span>
    </div>
    <div v-if="menuOpen" class="fixed inset-0 z-50 flex md:hidden" role="dialog" aria-modal="true" :aria-label="t('nav.label')" @keydown.esc="menuOpen = false">
      <div class="relative h-full w-72 max-w-[85vw]">
        <WorkspaceSidebar :key="ws" :workspace-id="ws" />
        <button type="button" class="absolute top-3 right-3 rounded-md p-2 text-sidebar-strong hover:bg-sidebar-active" :aria-label="t('nav.closeMenu')" @click="menuOpen = false">
          <X class="size-5" aria-hidden="true" />
        </button>
      </div>
      <button type="button" class="flex-1 bg-black/40" :aria-label="t('nav.closeMenu')" @click="menuOpen = false" />
    </div>

    <main v-if="editor" class="min-w-0 flex-1">
      <slot />
    </main>
    <main v-else class="min-w-0 flex-1 px-4 pb-6 md:px-10 md:pb-8">
      <div class="mx-auto max-w-6xl">
        <SeasonBar class="mb-6 border-b py-3 md:mb-8" />
        <slot />
      </div>
    </main>
    <ConfirmHost />
    <Toaster />
    <ReauthDialog />
  </div>

  <div v-else class="min-h-svh">
    <header class="bg-sidebar text-sidebar-strong">
      <div class="mx-auto flex h-15 max-w-6xl items-center gap-3 px-4">
        <NuxtLink to="/" class="flex items-center gap-2.5">
          <BrandMark :size="28" />
          <span class="font-mincho text-base tracking-[0.18em]">{{ t("app.name") }}</span>
        </NuxtLink>
        <div class="ml-auto">
          <AccountMenu v-if="me" placement="header" />
        </div>
      </div>
    </header>
    <div class="border-b bg-card">
      <SeasonBar class="mx-auto max-w-6xl px-4 py-2.5" />
    </div>
    <main class="mx-auto max-w-6xl px-4 py-8">
      <slot />
    </main>
    <ConfirmHost />
    <Toaster />
    <ReauthDialog />
  </div>
</template>
