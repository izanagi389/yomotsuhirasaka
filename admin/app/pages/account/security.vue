<script setup lang="ts">
// 本人の多要素認証: 認証アプリ（TOTP）の設定、パスキーの登録、復旧コードの作り直し、パスワードの変更。
// 全員への多要素認証の必須が有効なとき、設定が済むまでは、この画面だけが使える（Q197）。
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";

interface Identity {
  id: string;
  issuer: string;
  linkedAt: string;
  lastLoginAt: string | null;
}

interface Passkey {
  id: string;
  name: string;
  createdAt: string;
  lastUsedAt: string | null;
}

const { t, locale } = useI18n();
const api = useApi();
const { confirm } = useConfirm();
const { me, refresh } = useAuth();

const error = ref<unknown>(null);
const recoveryCodes = ref<string[]>([]);

// --- 認証アプリ（TOTP） ---
const totp = ref<{ secret: string; otpauthUrl: string } | null>(null);
const totpCode = ref("");
async function beginTotp() {
  error.value = null;
  try {
    totp.value = await api.post("/me/mfa/totp");
  } catch (e) {
    error.value = e;
  }
}
async function confirmTotp() {
  error.value = null;
  try {
    const res = await api.post<{ recoveryCodes: string[] }>("/me/mfa/totp/confirm", { code: totpCode.value.trim() });
    recoveryCodes.value = res.recoveryCodes;
    totp.value = null;
    totpCode.value = "";
    await refresh();
  } catch (e) {
    error.value = e;
  }
}

// --- パスキー ---
const passkeys = ref<Passkey[]>([]);
const passkeyName = ref("");
async function loadPasskeys() {
  try {
    passkeys.value = await api.get<Passkey[]>("/me/mfa/passkeys");
  } catch (e) {
    error.value = e;
  }
}
async function addPasskey() {
  error.value = null;
  try {
    const challenge = await api.post<{ publicKey: Record<string, unknown> }>("/me/mfa/passkeys", { name: passkeyName.value.trim() });
    const credential = await createPasskey(challenge);
    const res = await api.post<{ recoveryCodes: string[] }>("/me/mfa/passkeys/confirm", { credential });
    if (res.recoveryCodes.length > 0) recoveryCodes.value = res.recoveryCodes;
    passkeyName.value = "";
    await Promise.all([loadPasskeys(), refresh()]);
  } catch (e) {
    error.value = e;
  }
}
async function removePasskey(id: string) {
  const name = passkeys.value.find((p) => p.id === id)?.name ?? "";
  const ok = await confirm({
    title: t("confirmations.passkeyRemove.title", { name }),
    description: t("confirmations.passkeyRemove.description"),
    confirmLabel: t("common.remove"),
    destructive: true,
  });
  if (!ok) return;
  error.value = null;
  try {
    await api.del(`/me/mfa/passkeys/${id}`);
    await Promise.all([loadPasskeys(), refresh()]);
  } catch (e) {
    error.value = e;
  }
}

// --- 外部のアカウント（OIDC/SSO）のひも付け ---
const sso = useSso();
const identities = ref<Identity[]>([]);
async function loadIdentities() {
  try {
    identities.value = await api.get<Identity[]>("/me/identities");
  } catch (e) {
    error.value = e;
  }
}
async function linkIdentity() {
  error.value = null;
  try {
    await sso.start("/me/identities/oidc/start", undefined, "/account/security");
  } catch (e) {
    error.value = e;
  }
}

async function unlinkIdentity(id: string) {
  const issuer = identities.value.find((i) => i.id === id)?.issuer ?? "";
  const ok = await confirm({
    title: t("confirmations.identityUnlink.title"),
    description: t("confirmations.identityUnlink.description", { issuer }),
    confirmLabel: t("security.identityUnlink"),
    destructive: true,
  });
  if (!ok) return;
  error.value = null;
  try {
    await api.del(`/me/identities/${encodeURIComponent(id)}`);
    await loadIdentities();
  } catch (e) {
    error.value = e;
  }
}

// --- 復旧コード ---
async function regenerateCodes() {
  const ok = await confirm({
    title: t("confirmations.recoveryRegenerate.title"),
    description: t("confirmations.recoveryRegenerate.description"),
    confirmLabel: t("security.recoveryRegenerate"),
    destructive: true,
  });
  if (!ok) return;
  error.value = null;
  try {
    recoveryCodes.value = (await api.post<{ recoveryCodes: string[] }>("/me/mfa/recovery-codes")).recoveryCodes;
  } catch (e) {
    error.value = e;
  }
}

// --- パスワード ---
const currentPassword = ref("");
const newPassword = ref("");
const passwordChanged = ref(false);
async function changePassword() {
  error.value = null;
  passwordChanged.value = false;
  try {
    await api.post("/me/password", { currentPassword: currentPassword.value, newPassword: newPassword.value });
    currentPassword.value = "";
    newPassword.value = "";
    passwordChanged.value = true;
  } catch (e) {
    error.value = e;
  }
}

onMounted(() => {
  loadPasskeys();
  sso.load().then((i) => {
    if (i.enabled) loadIdentities();
  });
});
</script>

<template>
  <div class="grid max-w-2xl gap-6">
    <PageHeader :title="t('account.nav.security')" :description="t('security.title')" />
    <AccountNav />
    <Alert v-if="me?.mfaSetupRequired">
      <AlertTitle>{{ t("security.requiredTitle") }}</AlertTitle>
      <AlertDescription>{{ t("security.requiredDescription") }}</AlertDescription>
    </Alert>
    <Alert v-if="me?.passkeyRequired">
      <AlertTitle>{{ t("security.passkeyRequiredTitle") }}</AlertTitle>
      <AlertDescription>{{ t("security.passkeyRequiredDescription") }}</AlertDescription>
    </Alert>
    <FormError :error="error" />

    <Card v-if="recoveryCodes.length > 0">
      <CardHeader>
        <CardTitle>{{ t("security.recoveryCodesTitle") }}</CardTitle>
        <CardDescription>{{ t("security.recoveryCodesDescription") }}</CardDescription>
      </CardHeader>
      <CardContent>
        <ul class="grid grid-cols-2 gap-2 font-mono text-sm">
          <li v-for="c in recoveryCodes" :key="c">{{ c }}</li>
        </ul>
      </CardContent>
    </Card>

    <!-- SSOのセッションでは、多要素認証はIdPに任せ、本人の確認はパスキーで行う（Q131）。認証アプリとパスワードは出さない -->
    <Card v-if="!me?.sso">
      <CardHeader>
        <CardTitle>{{ t("security.totpTitle") }}</CardTitle>
        <CardDescription>{{ me?.mfaEnabled ? t("security.mfaEnabled") : t("security.totpDescription") }}</CardDescription>
      </CardHeader>
      <CardContent class="grid gap-4">
        <Button v-if="!totp" variant="outline" class="justify-self-start" @click="beginTotp">{{ t("security.totpBegin") }}</Button>
        <form v-else class="grid gap-4" @submit.prevent="confirmTotp">
          <p class="text-sm">{{ t("security.totpScan") }}</p>
          <QrCode :value="totp.otpauthUrl" :label="t('security.totpQrLabel')" class="justify-self-start" />
          <p class="text-sm">{{ t("security.totpManual") }}</p>
          <code class="rounded bg-muted p-2 font-mono text-sm break-all">{{ totp.secret }}</code>
          <a :href="totp.otpauthUrl" class="text-sm underline">{{ t("security.totpOpenApp") }}</a>
          <div class="grid gap-2">
            <Label for="totp-code">{{ t("auth.totpCode") }}</Label>
            <Input id="totp-code" v-model="totpCode" autocomplete="one-time-code" inputmode="numeric" required />
          </div>
          <Button type="submit" class="justify-self-start">{{ t("security.totpConfirm") }}</Button>
        </form>
      </CardContent>
    </Card>

    <Card>
      <CardHeader>
        <CardTitle>{{ t("security.passkeysTitle") }}</CardTitle>
        <CardDescription>{{ t("security.passkeysDescription") }}</CardDescription>
      </CardHeader>
      <CardContent class="grid gap-4">
        <ul class="grid gap-2">
          <li v-for="p in passkeys" :key="p.id" class="flex items-center justify-between gap-2 text-sm">
            <span>{{ p.name }}</span>
            <Button variant="outline" size="sm" class="text-destructive hover:text-destructive" @click="removePasskey(p.id)">{{ t("common.remove") }}</Button>
          </li>
        </ul>
        <form class="flex gap-2" @submit.prevent="addPasskey">
          <Input v-model="passkeyName" :placeholder="t('security.passkeyName')" required maxlength="100" />
          <Button type="submit" variant="outline">{{ t("security.passkeyAdd") }}</Button>
        </form>
      </CardContent>
    </Card>

    <Card v-if="sso.info.value?.enabled">
      <CardHeader>
        <CardTitle>{{ t("security.identitiesTitle") }}</CardTitle>
        <CardDescription>{{ t("security.identitiesDescription", { name: sso.info.value.displayName }) }}</CardDescription>
      </CardHeader>
      <CardContent class="grid gap-4">
        <ul v-if="identities.length > 0" class="grid gap-2 text-sm">
          <li v-for="i in identities" :key="i.id" class="flex flex-wrap justify-between gap-2">
            <span class="break-all">{{ i.issuer }}</span>
            <span class="text-muted-foreground">{{ t("security.identityLinkedAt", { at: formatDateTime(i.linkedAt, locale) }) }}</span>
            <Button variant="outline" size="sm" class="text-destructive hover:text-destructive" @click="unlinkIdentity(i.id)">{{ t("security.identityUnlink") }}</Button>
          </li>
        </ul>
        <Button v-else variant="outline" class="justify-self-start" @click="linkIdentity">
          {{ t("security.identityLink", { name: sso.info.value.displayName }) }}
        </Button>
      </CardContent>
    </Card>

    <Card v-if="me?.mfaEnabled">
      <CardHeader>
        <CardTitle>{{ t("security.recoveryTitle") }}</CardTitle>
        <CardDescription>{{ t("security.recoveryDescription") }}</CardDescription>
      </CardHeader>
      <CardContent>
        <Button variant="outline" @click="regenerateCodes">{{ t("security.recoveryRegenerate") }}</Button>
      </CardContent>
    </Card>

    <Card v-if="!me?.sso">
      <CardHeader>
        <CardTitle>{{ t("security.passwordTitle") }}</CardTitle>
        <CardDescription>{{ passwordChanged ? t("security.passwordChanged") : t("auth.passwordRules") }}</CardDescription>
      </CardHeader>
      <CardContent>
        <form class="grid gap-4" @submit.prevent="changePassword">
          <div class="grid gap-2">
            <Label for="current-password">{{ t("security.currentPassword") }}</Label>
            <Input id="current-password" v-model="currentPassword" type="password" autocomplete="current-password" required />
          </div>
          <div class="grid gap-2">
            <Label for="new-password">{{ t("auth.newPassword") }}</Label>
            <Input id="new-password" v-model="newPassword" type="password" autocomplete="new-password" minlength="12" required />
          </div>
          <Button type="submit" class="justify-self-start">{{ t("common.save") }}</Button>
        </form>
      </CardContent>
    </Card>
  </div>
</template>
