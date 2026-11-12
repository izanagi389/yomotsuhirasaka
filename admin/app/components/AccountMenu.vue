<script setup lang="ts">
// 本人のメニュー（アカウントの設定、言語、サーバーの管理、ログアウト）。サイドバーの下と、上部の帯に置く。
import { ChevronsUpDown, KeyRound, Languages, LogOut, Monitor, Server, ShieldCheck, UserRound } from "@lucide/vue";

import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";

withDefaults(defineProps<{ placement?: "sidebar" | "header"; compact?: boolean }>(), { placement: "sidebar", compact: false });

const { me, logout } = useAuth();
const { t, locale, locales, setLocale } = useI18n();
const api = useApi();

const name = computed(() => me.value?.displayName || me.value?.email || "");
const initial = computed(() => Array.from(name.value)[0]?.toUpperCase() ?? "?");

/** 言語を切り替え、本人の設定にも保存する。 */
async function changeLocale(code: string) {
  if (code !== "ja" && code !== "en") return;
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
  <DropdownMenu v-if="me">
    <DropdownMenuTrigger
      class="flex min-h-11 items-center gap-2.5 rounded-lg p-2 text-left text-sm outline-none focus-visible:ring-2 focus-visible:ring-ring"
      :class="placement === 'sidebar' ? 'w-full text-sidebar-strong hover:bg-sidebar-active' : 'text-inherit hover:bg-white/10'"
      :aria-label="t('accountMenu.open')"
      data-testid="account-menu"
    >
      <span aria-hidden="true" class="flex size-7.5 shrink-0 items-center justify-center rounded-full bg-[#2f6fb5] text-xs text-white">{{ initial }}</span>
      <span class="grid min-w-0 leading-tight" :class="compact ? 'sr-only' : placement === 'header' ? 'max-sm:hidden' : ''">
        <span class="truncate text-[13px]">{{ name }}</span>
        <span v-if="me.isServerAdmin" class="truncate text-[11px] text-sidebar-muted">{{ t("accountMenu.serverAdmin") }}</span>
      </span>
      <ChevronsUpDown v-if="!compact" class="ml-auto size-4 text-sidebar-muted" aria-hidden="true" />
    </DropdownMenuTrigger>
    <DropdownMenuContent :side="placement === 'sidebar' ? 'top' : 'bottom'" align="start" class="w-64">
      <DropdownMenuLabel class="grid font-normal">
        <span class="truncate font-medium">{{ name }}</span>
        <span class="truncate text-xs text-muted-foreground">{{ me.email }}</span>
      </DropdownMenuLabel>
      <DropdownMenuSeparator />
      <DropdownMenuItem @select="navigateTo('/account/security')"><ShieldCheck aria-hidden="true" />{{ t("account.nav.security") }}</DropdownMenuItem>
      <template v-if="!me.mfaSetupRequired">
        <DropdownMenuItem @select="navigateTo('/account/profile')"><UserRound aria-hidden="true" />{{ t("account.nav.profile") }}</DropdownMenuItem>
        <DropdownMenuItem @select="navigateTo('/account/sessions')"><Monitor aria-hidden="true" />{{ t("account.nav.sessions") }}</DropdownMenuItem>
        <DropdownMenuItem @select="navigateTo('/account/tokens')"><KeyRound aria-hidden="true" />{{ t("account.nav.tokens") }}</DropdownMenuItem>
      </template>
      <template v-if="me.isServerAdmin && !me.mfaSetupRequired">
        <DropdownMenuSeparator />
        <DropdownMenuItem @select="navigateTo('/server')"><Server aria-hidden="true" />{{ t("server.title") }}</DropdownMenuItem>
      </template>
      <DropdownMenuSeparator />
      <DropdownMenuLabel class="flex items-center gap-2 text-xs font-normal text-muted-foreground"><Languages class="size-3.5" aria-hidden="true" />{{ t("app.language") }}</DropdownMenuLabel>
      <DropdownMenuRadioGroup :model-value="locale" @update:model-value="(v) => changeLocale(String(v))">
        <DropdownMenuRadioItem v-for="l in locales" :key="l.code" :value="l.code">{{ l.name }}</DropdownMenuRadioItem>
      </DropdownMenuRadioGroup>
      <DropdownMenuSeparator />
      <DropdownMenuItem data-testid="logout" @select="logout"><LogOut aria-hidden="true" />{{ t("auth.logout") }}</DropdownMenuItem>
    </DropdownMenuContent>
  </DropdownMenu>
</template>
