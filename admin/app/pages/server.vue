<script setup lang="ts">
// サーバー管理者の画面: 利用者（リセットの発行）とインスタンス全体の監査ログ。権限の判断はサーバー。
// サーバー管理者自身のリセットは、どの経路でもできない（サーバーのCLIで行う。INV-AUTH-10）。
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import AuditPanel from "~/components/admin/AuditPanel.vue";
import type { Issued } from "~/utils/admin";

interface UserInfo {
  id: string;
  email: string;
  isServerAdmin: boolean;
  disabled: boolean;
  mfaEnabled: boolean;
}

const { t } = useI18n();
const api = useApi();
const { me } = useAuth();
const tab = ref("users");
const { data: users, error } = await useAsyncData("server:users", () =>
  me.value?.isServerAdmin ? api.get<UserInfo[]>("/users") : Promise.resolve([] as UserInfo[]),
);
const people = computed(() => Object.fromEntries((users.value ?? []).map((u) => [u.id, u.email])));

const actionError = ref<unknown>(null);
const issued = ref<{ email: string; kind: "password" | "mfa"; result: Issued } | null>(null);
async function issueReset(u: UserInfo, kind: "password" | "mfa") {
  actionError.value = null;
  issued.value = null;
  try {
    const result = await api.post<Issued>(`/users/${u.id}/${kind === "password" ? "password-reset" : "mfa-reset"}`);
    issued.value = { email: u.email, kind, result };
  } catch (e) {
    actionError.value = e;
  }
}
</script>

<template>
  <div class="grid gap-6 text-sm">
    <h1 class="text-2xl font-semibold">{{ t("server.title") }}</h1>
    <p v-if="!me?.isServerAdmin" class="text-muted-foreground">{{ t("errors.forbidden") }}</p>
    <Tabs v-else v-model="tab">
      <TabsList>
        <TabsTrigger value="users">{{ t("server.users") }}</TabsTrigger>
        <TabsTrigger value="audit">{{ t("adminTabs.audit") }}</TabsTrigger>
      </TabsList>
      <TabsContent value="users" class="mt-4 grid gap-4">
        <FormError :error="error ?? actionError" />
        <template v-if="issued">
          <p v-if="issued.result.delivery === 'email'" class="rounded-md border p-3" role="status">
            {{ t(issued.kind === "password" ? "members.passwordResetSent" : "members.mfaResetSent", { email: issued.email }) }}
          </p>
          <SecretOnce
            v-else-if="issued.result.resetUrl"
            :value="issued.result.resetUrl"
            :label="t(issued.kind === 'password' ? 'members.passwordResetLink' : 'members.mfaResetLink', { email: issued.email })"
            @dismiss="issued = null"
          />
        </template>
        <ul class="grid gap-2">
          <li v-for="u in users ?? []" :key="u.id" class="flex flex-wrap items-center gap-2 rounded-md border p-3">
            <span class="font-medium">{{ u.email }}</span>
            <Badge v-if="u.isServerAdmin">{{ t("roles.serverAdmin") }}</Badge>
            <Badge v-if="u.disabled" variant="destructive">{{ t("server.disabled") }}</Badge>
            <Badge v-if="!u.mfaEnabled" variant="outline">{{ t("members.noMfa") }}</Badge>
            <div v-if="!u.isServerAdmin" class="ml-auto flex gap-2">
              <Button size="sm" variant="outline" @click="issueReset(u, 'password')">{{ t("members.issuePasswordReset") }}</Button>
              <Button size="sm" variant="outline" :disabled="!u.mfaEnabled" @click="issueReset(u, 'mfa')">{{ t("members.issueMfaReset") }}</Button>
            </div>
            <span v-else class="ml-auto text-muted-foreground">{{ t("server.cliOnly") }}</span>
          </li>
        </ul>
      </TabsContent>
      <TabsContent value="audit" class="mt-4"><AuditPanel :people="people" /></TabsContent>
    </Tabs>
  </div>
</template>
