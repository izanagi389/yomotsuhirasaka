<script setup lang="ts">
import { Button } from "@/components/ui/button";

const { me, logout } = useAuth();
const { t, locale, locales, setLocale } = useI18n();
const api = useApi();

/** 言語を切り替え、本人の設定にも保存する。 */
async function changeLocale(code: "ja" | "en") {
  await setLocale(code);
  if (me.value) {
    try {
      await api.patch("/me", { locale: code });
    } catch {
      // 保存できなくても、この画面の言語は切り替わっている
    }
  }
}
</script>

<template>
  <div class="min-h-svh">
    <header class="border-b">
      <div class="mx-auto flex h-14 max-w-6xl items-center gap-4 px-4">
        <NuxtLink to="/" class="font-semibold">{{ t("app.name") }}</NuxtLink>
        <div class="ml-auto flex items-center gap-2">
          <select
            class="h-8 rounded-md border bg-background px-2 text-sm"
            :aria-label="t('app.language')"
            :value="locale"
            @change="changeLocale(($event.target as HTMLSelectElement).value as 'ja' | 'en')"
          >
            <option v-for="l in locales" :key="l.code" :value="l.code">{{ l.name }}</option>
          </select>
          <template v-if="me">
            <NuxtLink to="/account/security" class="text-sm text-muted-foreground hover:text-foreground">{{ me.email }}</NuxtLink>
            <Button variant="outline" size="sm" @click="logout">{{ t("auth.logout") }}</Button>
          </template>
        </div>
      </div>
    </header>
    <main class="mx-auto max-w-6xl px-4 py-8">
      <slot />
    </main>
    <ReauthDialog />
  </div>
</template>
