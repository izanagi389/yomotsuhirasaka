<script setup lang="ts">
// 所属者（Q76, Q101, Q190）。ロールと閲覧の権限の変更、重い権限の付与、所属から外す、パスワード・多要素認証のリセットの発行。
// 重い権限は、ロールの既定で持つものは「ロールの既定」と出し、付与できるロール（GRANTABLE_TO）にだけスイッチを出す。
// どれも管理者・サーバー管理者だけで、再認証が要る（サーバーが求めたらダイアログで確認して送り直す）。
// 最後の管理者は外せない・ロールを下げられない（サーバーが last_admin を返す）。
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Switch } from "@/components/ui/switch";
import { GRANTABLE_TO, SERVER_ADMIN_GRANTS_ONLY, type Issued, type MemberInfo, ROLES, type Role } from "~/utils/admin";
import { HEAVY_PERMISSIONS, type HeavyPermission } from "~/utils/content";

const props = defineProps<{ workspaceId: string }>();
const { t, locale } = useI18n();
const api = useApi();
const { me } = useAuth();
const base = computed(() => `/workspaces/${props.workspaceId}/members`);
const { data: members, error, refresh } = useAsyncData(`members:${props.workspaceId}`, () => api.get<MemberInfo[]>(base.value));

const actionError = ref<unknown>(null);
const busy = ref(false);
async function change(m: MemberInfo, body: { role?: Role; canViewMembers?: boolean; grants?: HeavyPermission[] }) {
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

// 重い権限（行を開いて切り替える）
const openGrants = ref<string | null>(null);
/** 表に出す重い権限: ロールの既定で持つもの、付与できるもの、すでに付与したもの。
 *  サーバー管理者だけが付与できるもの（テンプレートの切り替え）は、ほかの人には付与済みのときだけ、変えられない形で出す。 */
function heavyRows(m: MemberInfo) {
  return HEAVY_PERMISSIONS.flatMap((p) => {
    const granted = m.grants.includes(p);
    const locked = SERVER_ADMIN_GRANTS_ONLY.includes(p) && !me.value?.isServerAdmin;
    if (m.permissions.includes(p) && !granted) return [{ p, byRole: true, granted: false, locked }];
    if (granted || (!locked && GRANTABLE_TO[p].includes(m.role))) return [{ p, byRole: false, granted, locked }];
    return [];
  });
}
function toggleGrant(m: MemberInfo, p: HeavyPermission, on: boolean) {
  const grants = on ? [...new Set([...m.grants, p])] : m.grants.filter((g) => g !== p);
  return change(m, { grants });
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
const { confirm } = useConfirm();
async function issueReset(m: MemberInfo, kind: "password" | "mfa") {
  const ok = await confirm({
    title: t(kind === "password" ? "confirmations.passwordReset.title" : "confirmations.mfaReset.title", { email: m.email }),
    description: t(kind === "password" ? "confirmations.passwordReset.description" : "confirmations.mfaReset.description"),
    confirmLabel: t(kind === "password" ? "members.issuePasswordReset" : "members.issueMfaReset"),
  });
  if (!ok) return;
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
            <th class="px-3 py-2 font-medium">{{ t("members.heavy") }}</th>
            <th class="px-3 py-2 font-medium">{{ t("members.joinedAt") }}</th>
            <th class="px-3 py-2" />
          </tr>
        </thead>
        <tbody>
          <template v-for="m in members ?? []" :key="m.userId">
            <tr class="border-t">
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
              <td class="px-3 py-2">
                <Button
                  size="sm"
                  variant="outline"
                  :aria-expanded="openGrants === m.userId"
                  :aria-label="t('members.heavyOf', { email: m.email })"
                  :data-testid="`heavy-${m.userId}`"
                  @click="openGrants = openGrants === m.userId ? null : m.userId"
                >
                  {{ t("members.heavyCount", { n: m.permissions.length }) }}
                </Button>
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
                    <DropdownMenuItem variant="destructive" @select="removing = m">{{ t("members.remove") }}</DropdownMenuItem>
                  </DropdownMenuContent>
                </DropdownMenu>
              </td>
            </tr>
            <tr v-if="openGrants === m.userId" class="bg-muted/30">
              <td colspan="6" class="px-3 py-3">
                <p class="mb-2 text-muted-foreground">{{ t("members.heavyHelp") }}</p>
                <ul class="grid gap-2">
                  <li v-for="row in heavyRows(m)" :key="row.p" class="flex items-start gap-3">
                    <Switch
                      v-if="!row.byRole"
                      :id="`grant-${m.userId}-${row.p}`"
                      :model-value="row.granted"
                      :disabled="busy || row.locked"
                      @update:model-value="(v) => toggleGrant(m, row.p, v as boolean)"
                    />
                    <Badge v-else variant="secondary">{{ t("members.byRole") }}</Badge>
                    <label :for="`grant-${m.userId}-${row.p}`" class="grid gap-0.5">
                      <span class="font-medium">{{ t(`permissions.${row.p}.label`) }}</span>
                      <span class="text-muted-foreground">{{ t(`permissions.${row.p}.help`) }}</span>
                    </label>
                  </li>
                </ul>
              </td>
            </tr>
          </template>
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
