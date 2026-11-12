<script setup lang="ts">
// 自分のセッション（ログイン中の端末）、ログインの履歴（保存期間の内側だけ）、ひも付けた外部のアカウント（OIDC）。
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";

interface SessionSummary {
  id: string;
  createdAt: string;
  lastSeenAt: string;
  expiresAt: string;
  current: boolean;
}
interface LoginEntry {
  at: string;
  ip: string;
  browser: string;
  country: string | null;
  newDevice: boolean;
}
interface Identity {
  id: string;
  issuer: string;
  linkedAt: string;
  lastLoginAt: string | null;
}

const { t, locale } = useI18n();
const api = useApi();
const { data: sessions, error, refresh } = await useAsyncData("me:sessions", () => api.get<SessionSummary[]>("/me/sessions"));
const { data: history } = await useAsyncData("me:login-history", () => api.get<LoginEntry[]>("/me/login-history"));
const { data: identities } = await useAsyncData("me:identities", () => api.get<Identity[]>("/me/identities"));

const actionError = ref<unknown>(null);
async function revoke(s: SessionSummary) {
  actionError.value = null;
  try {
    await api.del(`/me/sessions/${s.id}`);
    await refresh();
  } catch (e) {
    actionError.value = e;
  }
}
</script>

<template>
  <div class="grid max-w-3xl gap-6 text-sm">
    <PageHeader :title="t('account.nav.sessions')" :description="t('security.title')" />
    <AccountNav />
    <FormError :error="error ?? actionError" />

    <section class="grid gap-2">
      <h2 class="text-base font-medium">{{ t("account.sessions.title") }}</h2>
      <p class="text-muted-foreground">{{ t("account.sessions.help") }}</p>
      <ul class="grid gap-2">
        <li v-for="s in sessions ?? []" :key="s.id" class="flex flex-wrap items-center gap-3 rounded-md border p-3">
          <Badge v-if="s.current">{{ t("account.sessions.current") }}</Badge>
          <span>{{ t("account.sessions.started", { at: formatDateTime(s.createdAt, locale) }) }}</span>
          <span class="text-muted-foreground">{{ t("account.sessions.lastSeen", { at: formatDateTime(s.lastSeenAt, locale) }) }}</span>
          <Button v-if="!s.current" size="sm" variant="outline" class="ml-auto" @click="revoke(s)">{{ t("account.sessions.revoke") }}</Button>
        </li>
      </ul>
    </section>

    <section class="grid gap-2">
      <h2 class="text-base font-medium">{{ t("account.history.title") }}</h2>
      <p v-if="history && history.length === 0" class="text-muted-foreground">{{ t("account.history.none") }}</p>
      <div v-else class="overflow-x-auto rounded-md border">
        <table class="w-full">
          <thead class="bg-muted/50 text-left">
            <tr>
              <th class="px-3 py-2 font-medium">{{ t("account.history.at") }}</th>
              <th class="px-3 py-2 font-medium">{{ t("account.history.browser") }}</th>
              <th class="px-3 py-2 font-medium">{{ t("account.history.ip") }}</th>
              <th class="px-3 py-2 font-medium">{{ t("account.history.country") }}</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="(h, i) in history ?? []" :key="i" class="border-t">
              <td class="px-3 py-2">{{ formatDateTime(h.at, locale) }} <Badge v-if="h.newDevice" variant="outline">{{ t("account.history.newDevice") }}</Badge></td>
              <td class="px-3 py-2">{{ h.browser }}</td>
              <td class="px-3 py-2 font-mono text-xs">{{ h.ip }}</td>
              <td class="px-3 py-2">{{ h.country ?? "-" }}</td>
            </tr>
          </tbody>
        </table>
      </div>
    </section>

    <section class="grid gap-2">
      <h2 class="text-base font-medium">{{ t("account.identities.title") }}</h2>
      <p v-if="identities && identities.length === 0" class="text-muted-foreground">{{ t("account.identities.none") }}</p>
      <ul class="grid gap-2">
        <li v-for="i in identities ?? []" :key="i.id" class="flex flex-wrap gap-3 rounded-md border p-3">
          <span class="break-all font-medium">{{ i.issuer }}</span>
          <span class="text-muted-foreground">{{ t("account.identities.linked", { at: formatDateTime(i.linkedAt, locale) }) }}</span>
        </li>
      </ul>
    </section>
  </div>
</template>
