<script setup lang="ts">
// ログイン: 1段目はメールアドレスとパスワード、多要素認証を設定した人は2段目（TOTP・復旧コード・パスキー）。
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";

definePageMeta({ layout: "auth" });

const { t } = useI18n();
const api = useApi();
const { refresh } = useAuth();
const route = useRoute();

const step = ref<"password" | "mfa">("password");
const email = ref("");
const password = ref("");
const code = ref("");
const useRecovery = ref(false);
const error = ref<unknown>(null);
const busy = ref(false);

interface LoginResponse {
  mfaRequired: boolean;
  csrfToken?: string;
}

/** 戻り先はこの管理画面の中のパスだけ（別のサイトへの転送に使わせない）。 */
function next(): string {
  const n = route.query.next;
  return typeof n === "string" && n.startsWith("/") && !n.startsWith("//") ? n : "/";
}

async function done(res: LoginResponse) {
  setCsrfToken(res.csrfToken ?? null);
  await refresh();
  await navigateTo(next());
}

async function submitPassword() {
  busy.value = true;
  error.value = null;
  try {
    const res = await api.post<LoginResponse>("/session", { email: email.value, password: password.value }, { noReauth: true });
    password.value = "";
    if (res.mfaRequired) step.value = "mfa";
    else await done(res);
  } catch (e) {
    error.value = e;
  } finally {
    busy.value = false;
  }
}

async function submitCode() {
  busy.value = true;
  error.value = null;
  try {
    const body = useRecovery.value ? { recoveryCode: code.value.trim() } : { totpCode: code.value.trim() };
    await done(await api.post<LoginResponse>("/session/mfa", body, { noReauth: true }));
  } catch (e) {
    error.value = e;
  } finally {
    busy.value = false;
  }
}

async function submitPasskey() {
  busy.value = true;
  error.value = null;
  try {
    const challenge = await api.post<{ publicKey: Record<string, unknown> }>("/session/mfa/passkey", undefined, { noReauth: true });
    const passkey = await usePasskey(challenge);
    await done(await api.post<LoginResponse>("/session/mfa", { passkey }, { noReauth: true }));
  } catch (e) {
    error.value = e;
  } finally {
    busy.value = false;
  }
}
</script>

<template>
  <Card>
    <CardHeader>
      <CardTitle>{{ t("auth.loginTitle") }}</CardTitle>
      <CardDescription>{{ step === "password" ? t("auth.loginDescription") : t("auth.mfaDescription") }}</CardDescription>
    </CardHeader>
    <CardContent>
      <form v-if="step === 'password'" class="grid gap-4" @submit.prevent="submitPassword">
        <FormError :error="error" />
        <div class="grid gap-2">
          <Label for="email">{{ t("auth.email") }}</Label>
          <Input id="email" v-model="email" type="email" autocomplete="username" required />
        </div>
        <div class="grid gap-2">
          <Label for="password">{{ t("auth.password") }}</Label>
          <Input id="password" v-model="password" type="password" autocomplete="current-password" required />
        </div>
        <Button type="submit" :disabled="busy">{{ t("auth.login") }}</Button>
        <NuxtLink to="/forgot-password" class="text-center text-sm text-muted-foreground hover:underline">{{ t("auth.forgotPassword") }}</NuxtLink>
      </form>
      <form v-else class="grid gap-4" @submit.prevent="submitCode">
        <FormError :error="error" />
        <div class="grid gap-2">
          <Label for="code">{{ useRecovery ? t("auth.recoveryCode") : t("auth.totpCode") }}</Label>
          <Input id="code" v-model="code" autocomplete="one-time-code" :inputmode="useRecovery ? undefined : 'numeric'" required />
        </div>
        <Button type="submit" :disabled="busy">{{ t("auth.verify") }}</Button>
        <div class="flex flex-wrap justify-between gap-2 text-sm">
          <Button type="button" variant="link" class="h-auto p-0" @click="useRecovery = !useRecovery">
            {{ useRecovery ? t("auth.useTotp") : t("auth.useRecoveryCode") }}
          </Button>
          <Button type="button" variant="link" class="h-auto p-0" :disabled="busy" @click="submitPasskey">{{ t("auth.usePasskey") }}</Button>
        </div>
      </form>
    </CardContent>
  </Card>
</template>
