<script setup lang="ts">
// 多要素認証のリセットのリンクから、多要素認証を解除する。その後、パスワードでログインして設定し直す。
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";

definePageMeta({ layout: "auth" });
const { t } = useI18n();
const api = useApi();
const token = useLinkToken();
const done = ref(false);
const error = ref<unknown>(null);

async function submit() {
  error.value = null;
  try {
    await api.post("/mfa-resets/complete", { token: token.value }, { noReauth: true });
    done.value = true;
  } catch (e) {
    error.value = e;
  }
}
</script>

<template>
  <Card>
    <CardHeader>
      <CardTitle>{{ t("reset.mfaTitle") }}</CardTitle>
      <CardDescription>{{ done ? t("reset.mfaDone") : t("reset.mfaDescription") }}</CardDescription>
    </CardHeader>
    <CardContent class="grid gap-4">
      <p v-if="!token && !done" class="text-sm text-destructive">{{ t("errors.invalid_token") }}</p>
      <template v-else-if="!done">
        <FormError :error="error" />
        <Button @click="submit">{{ t("reset.mfaConfirm") }}</Button>
      </template>
      <NuxtLink to="/login" class="block text-center text-sm text-muted-foreground hover:underline">{{ t("auth.backToLogin") }}</NuxtLink>
    </CardContent>
  </Card>
</template>
