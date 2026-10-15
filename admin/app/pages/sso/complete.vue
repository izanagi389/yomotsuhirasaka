<script setup lang="ts">
// IdPから戻った画面（サーバーの戻り先が、結果をここへ転送する）。
// ログイン・招待の承諾なら、CSRFトークンを一度だけ受け取る。ひも付け・再認証なら、元の画面に戻る。
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";

definePageMeta({ layout: "auth" });

const { t, te } = useI18n();
const api = useApi();
const route = useRoute();
const { refresh } = useAuth();

const purpose = computed(() => (typeof route.query.purpose === "string" ? route.query.purpose : ""));
const failure = computed(() => (typeof route.query.error === "string" ? route.query.error : null));
const error = ref<unknown>(null);
const busy = ref(true);

const failureMessage = computed(() => {
  if (!failure.value) return null;
  const key = `sso.errors.${failure.value}`;
  return te(key) ? t(key) : t("sso.errors.generic");
});

/** 失敗したときに戻る画面。 */
const backTo = computed(() => (purpose.value === "link" || purpose.value === "reauth" ? "/account/security" : "/login"));

onMounted(async () => {
  const returnTo = takeSsoReturn();
  if (failure.value) {
    busy.value = false;
    return;
  }
  try {
    if (purpose.value === "login" || purpose.value === "invite") {
      const res = await api.post<{ csrfToken: string }>("/session/oidc/complete", undefined, { noReauth: true });
      setCsrfToken(res.csrfToken);
      const me = await refresh();
      // パスキーのないSSOの管理者は、まずパスキーを登録する（Q131）
      await navigateTo(me?.passkeyRequired ? "/account/security" : (returnTo ?? "/"), { replace: true });
      return;
    }
    await refresh();
    await navigateTo(returnTo ?? "/account/security", { replace: true });
  } catch (e) {
    error.value = e;
    busy.value = false;
  }
});
</script>

<template>
  <Card>
    <CardHeader>
      <CardTitle>{{ t("sso.completeTitle") }}</CardTitle>
      <CardDescription v-if="busy">{{ t("sso.completing") }}</CardDescription>
    </CardHeader>
    <CardContent v-if="!busy" class="grid gap-4">
      <p v-if="failureMessage" role="alert" class="text-sm text-destructive">{{ failureMessage }}</p>
      <FormError :error="error" />
      <Button as-child variant="outline">
        <NuxtLink :to="backTo">{{ backTo === "/login" ? t("auth.backToLogin") : t("sso.backToAccount") }}</NuxtLink>
      </Button>
    </CardContent>
  </Card>
</template>
