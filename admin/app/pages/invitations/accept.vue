<script setup lang="ts">
// 招待の承諾。ログインしていれば、その利用者として承諾する。していなければ、パスワードを決めて新しいアカウントを作る。
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";

definePageMeta({ layout: "auth" });
const { t } = useI18n();
const api = useApi();
const { me } = useAuth();
const token = useLinkToken();
const password = ref("");
const done = ref(false);
const error = ref<unknown>(null);

async function accept() {
  error.value = null;
  try {
    if (me.value) await api.post("/me/invitations/accept", { token: token.value });
    else await api.post("/invitations/accept", { token: token.value, newPassword: password.value }, { noReauth: true });
    done.value = true;
  } catch (e) {
    error.value = e;
  }
}
</script>

<template>
  <Card>
    <CardHeader>
      <CardTitle>{{ t("invite.title") }}</CardTitle>
      <CardDescription>{{ done ? t("invite.done") : me ? t("invite.asMember", { email: me.email }) : t("invite.asNew") }}</CardDescription>
    </CardHeader>
    <CardContent class="grid gap-4">
      <p v-if="!token && !done" class="text-sm text-destructive">{{ t("errors.invalid_token") }}</p>
      <form v-else-if="!done" class="grid gap-4" @submit.prevent="accept">
        <FormError :error="error" />
        <div v-if="!me" class="grid gap-2">
          <Label for="password">{{ t("auth.newPassword") }}</Label>
          <Input id="password" v-model="password" type="password" autocomplete="new-password" minlength="12" required />
          <p class="text-xs text-muted-foreground">{{ t("auth.passwordRules") }}</p>
        </div>
        <Button type="submit">{{ t("invite.accept") }}</Button>
      </form>
      <NuxtLink :to="me ? '/' : '/login'" class="block text-center text-sm text-muted-foreground hover:underline">
        {{ me ? t("common.home") : t("auth.backToLogin") }}
      </NuxtLink>
    </CardContent>
  </Card>
</template>
