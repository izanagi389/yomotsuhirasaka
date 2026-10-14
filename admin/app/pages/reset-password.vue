<script setup lang="ts">
// リセットのリンクから、新しいパスワードを決める。
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";

definePageMeta({ layout: "auth" });
const { t } = useI18n();
const api = useApi();
const token = useLinkToken();
const password = ref("");
const done = ref(false);
const error = ref<unknown>(null);

async function submit() {
  error.value = null;
  try {
    await api.post("/password-resets/complete", { token: token.value, newPassword: password.value }, { noReauth: true });
    done.value = true;
  } catch (e) {
    error.value = e;
  }
}
</script>

<template>
  <Card>
    <CardHeader>
      <CardTitle>{{ t("reset.passwordTitle") }}</CardTitle>
      <CardDescription>{{ done ? t("reset.passwordDone") : t("auth.passwordRules") }}</CardDescription>
    </CardHeader>
    <CardContent>
      <p v-if="!token && !done" class="text-sm text-destructive">{{ t("errors.invalid_token") }}</p>
      <form v-else-if="!done" class="grid gap-4" @submit.prevent="submit">
        <FormError :error="error" />
        <div class="grid gap-2">
          <Label for="password">{{ t("auth.newPassword") }}</Label>
          <Input id="password" v-model="password" type="password" autocomplete="new-password" minlength="12" required />
        </div>
        <Button type="submit">{{ t("common.save") }}</Button>
      </form>
      <NuxtLink to="/login" class="mt-4 block text-center text-sm text-muted-foreground hover:underline">{{ t("auth.backToLogin") }}</NuxtLink>
    </CardContent>
  </Card>
</template>
