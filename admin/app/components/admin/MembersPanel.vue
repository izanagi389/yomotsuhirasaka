<script setup lang="ts">
// 所属者（Q76, Q101, Q190）。ロールと閲覧の権限の変更、所属から外す、パスワード・多要素認証のリセットの発行。
// どれも管理者・サーバー管理者だけで、再認証が要る（サーバーが求めたらダイアログで確認して送り直す）。
// 最後の管理者は外せない・ロールを下げられない（サーバーが last_admin を返す）。
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Switch } from "@/components/ui/switch";
import { type Issued, type MemberInfo, ROLES, type Role } from "~/utils/admin";

const props = defineProps<{ workspaceId: string }>();
const { t, locale } = useI18n();
const api = useApi();
const { me } = useAuth();
const base = computed(() => `/workspaces/${props.workspaceId}/members`);
const { data: members, error, refresh } = useAsyncData(`members:${props.workspaceId}`, () => api.get<MemberInfo[]>(base.value));

const actionError = ref<unknown>(null);
const busy = ref(false);
async function change(m: MemberInfo, body: { role?: Role; canViewMembers?: boolean }) {
  busy.value = true;
  actionError.value = null;
  try {
    await api.patch(`${base.value}/${m.userId}`, body);
  } catch (e) {
    actionError.value = e;
  } finally {
    busy.value = false;
    await refresh();
  }
}

// 外す
const removing = ref<MemberInfo | null>(null);
const removeError = ref<unknown>(null);
async function remove() {
  if (!removing.value) return;
  busy.value = true;
  removeError.value = null;
  try {
    await api.del(`${base.value}/${removing.value.userId}`);
    removing.value = null;
    await refresh();
  } catch (e) {
    removeError.value = e;
  } finally {
    busy.value = false;
  }
}

// リセットの発行（本人向けのリンク。SMTP がなければ一度だけ表示する）
const issued = ref<{ email: string; kind: "password" | "mfa"; result: Issued } | null>(null);
async function issueReset(m: MemberInfo, kind: "password" | "mfa") {
  actionError.value = null;
  issued.value = null;
  try {
    const result = await api.post<Issued>(`${base.value}/${m.userId}/${kind === "password" ? "password-reset" : "mfa-reset"}`);
    issued.value = { email: m.email, kind, result };
  } catch (e) {
    actionError.value = e;
  }
}
</script>

<template>
  <div class="grid gap-4 text-sm">
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
    <div class="overflow-x-auto rounded-md border">
      <table class="w-full">
        <thead class="bg-muted/50 text-left">
          <tr>
            <th class="px-3 py-2 font-medium">{{ t("auth.email") }}</th>
            <th class="px-3 py-2 font-medium">{{ t("members.role") }}</th>
            <th class="px-3 py-2 font-medium">{{ t("members.canViewMembers") }}</th>
            <th class="px-3 py-2 font-medium">{{ t("members.joinedAt") }}</th>
            <th class="px-3 py-2" />
          </tr>
        </thead>
        <tbody>
          <tr v-for="m in members ?? []" :key="m.userId" class="border-t">
            <td class="px-3 py-2">
              {{ m.email }}
              <Badge v-if="!m.mfaEnabled" variant="outline" class="ml-1">{{ t("members.noMfa") }}</Badge>
            </td>
            <td class="px-3 py-2">
              <select
                class="h-8 rounded-md border bg-background px-2"
                :value="m.role"
                :disabled="busy"
                :aria-label="t('members.roleOf', { email: m.email })"
                @change="change(m, { role: ($event.target as HTMLSelectElement).value as Role })"
              >
                <option v-for="r in ROLES" :key="r" :value="r">{{ t(`roles.${r}`) }}</option>
              </select>
            </td>
            <td class="px-3 py-2">
              <Switch
                v-if="m.role !== 'admin'"
                :model-value="m.canViewMembers"
                :disabled="busy"
                :aria-label="t('members.canViewMembersOf', { email: m.email })"
                @update:model-value="(v) => change(m, { canViewMembers: v as boolean })"
              />
            </td>
            <td class="px-3 py-2 text-muted-foreground">{{ formatDateTime(m.joinedAt, locale) }}</td>
            <td class="px-3 py-2 text-right">
              <DropdownMenu v-if="m.userId !== me?.id">
                <DropdownMenuTrigger as-child>
                  <Button size="sm" variant="ghost" :aria-label="t('members.actionsFor', { email: m.email })">…</Button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="end">
                  <DropdownMenuItem @select="issueReset(m, 'password')">{{ t("members.issuePasswordReset") }}</DropdownMenuItem>
                  <DropdownMenuItem :disabled="!m.mfaEnabled" @select="issueReset(m, 'mfa')">{{ t("members.issueMfaReset") }}</DropdownMenuItem>
                  <DropdownMenuItem class="text-destructive" @select="removing = m">{{ t("members.remove") }}</DropdownMenuItem>
                </DropdownMenuContent>
              </DropdownMenu>
            </td>
          </tr>
        </tbody>
      </table>
    </div>
    <ConfirmDialog
      :open="removing !== null"
      :title="t('members.removeTitle')"
      :description="t('members.removeDescription', { email: removing?.email ?? '' })"
      :confirm-label="t('members.remove')"
      destructive
      :busy="busy"
      :error="removeError"
      @update:open="(v) => !v && (removing = null)"
      @confirm="remove"
    />
  </div>
</template>
