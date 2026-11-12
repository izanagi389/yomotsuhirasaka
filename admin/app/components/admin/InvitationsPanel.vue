<script setup lang="ts">
// 招待（Q104, Q121, Q193, Q194）。所属者を加えるのは、招待の承諾だけ。
// SMTP を設定していればメールで送る。設定していなければ、リンクをこの画面で一度だけ表示する（その場合はサーバー管理者だけが招待できる）。
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { type InvitationSummary, type Issued, ROLES, type Role } from "~/utils/admin";

const props = defineProps<{ workspaceId: string }>();
const { t, locale } = useI18n();
const api = useApi();
const base = computed(() => `/workspaces/${props.workspaceId}/invitations`);
const { data: pending, error, refresh } = useAsyncData(`invitations:${props.workspaceId}`, () => api.get<InvitationSummary[]>(base.value));

const email = ref("");
const role = ref<Role>("author");
const busy = ref(false);
const actionError = ref<unknown>(null);
const issued = ref<{ email: string; result: Issued } | null>(null);
async function invite() {
  busy.value = true;
  actionError.value = null;
  issued.value = null;
  try {
    const result = await api.post<Issued>(base.value, { email: email.value.trim(), role: role.value });
    issued.value = { email: email.value.trim(), result };
    email.value = "";
    await refresh();
  } catch (e) {
    actionError.value = e;
  } finally {
    busy.value = false;
  }
}

const revoking = ref<InvitationSummary | null>(null);
const revokeError = ref<unknown>(null);
async function revoke() {
  if (!revoking.value) return;
  busy.value = true;
  revokeError.value = null;
  try {
    await api.del(`${base.value}/${revoking.value.id}`);
    revoking.value = null;
    await refresh();
  } catch (e) {
    revokeError.value = e;
  } finally {
    busy.value = false;
  }
}
</script>

<template>
  <div class="grid gap-6 text-sm">
    <form class="grid gap-3 rounded-md border p-4 sm:grid-cols-[1fr_auto_auto] sm:items-end" @submit.prevent="invite">
      <div class="grid gap-2">
        <Label for="invite-email">{{ t("auth.email") }}</Label>
        <Input id="invite-email" v-model="email" type="email" autocomplete="off" required />
      </div>
      <div class="grid gap-2">
        <Label for="invite-role">{{ t("members.role") }}</Label>
        <select id="invite-role" v-model="role" class="h-9 rounded-md border bg-background px-3">
          <option v-for="r in ROLES" :key="r" :value="r">{{ t(`roles.${r}`) }}</option>
        </select>
      </div>
      <Button type="submit" :disabled="busy || !email.trim()">{{ t("invitations.send") }}</Button>
    </form>
    <FormError :error="error ?? actionError" />
    <template v-if="issued">
      <p v-if="issued.result.delivery === 'email'" class="rounded-md border p-3" role="status">{{ t("invitations.sent", { email: issued.email }) }}</p>
      <SecretOnce
        v-else-if="issued.result.inviteUrl"
        :value="issued.result.inviteUrl"
        :label="t('invitations.link', { email: issued.email })"
        @dismiss="issued = null"
      />
    </template>

    <section class="grid gap-2">
      <h2 class="font-medium">{{ t("invitations.pending") }}</h2>
      <p v-if="pending && pending.length === 0" class="text-muted-foreground">{{ t("invitations.none") }}</p>
      <ul class="grid gap-2">
        <li v-for="i in pending ?? []" :key="i.id" class="flex flex-wrap items-center gap-3 rounded-md border p-3">
          <span class="font-medium">{{ i.email }}</span>
          <span>{{ t(`roles.${i.role}`) }}</span>
          <span class="text-muted-foreground">{{ t("invitations.expires", { at: formatDateTime(i.expiresAt, locale) }) }}</span>
          <Button size="sm" variant="outline" class="ml-auto text-destructive hover:text-destructive" @click="revoking = i">{{ t("invitations.revoke") }}</Button>
        </li>
      </ul>
    </section>
    <ConfirmDialog
      :open="revoking !== null"
      :title="t('invitations.revokeTitle')"
      :description="t('invitations.revokeDescription', { email: revoking?.email ?? '' })"
      :confirm-label="t('invitations.revoke')"
      destructive
      :busy="busy"
      :error="revokeError"
      @update:open="(v) => !v && (revoking = null)"
      @confirm="revoke"
    />
  </div>
</template>
