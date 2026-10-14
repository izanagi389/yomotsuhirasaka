<script setup lang="ts">
// パスワードを忘れたとき。応答は、登録の有無によらず同じ（13-security.md「リセットと招待のリンク」）。
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";

definePageMeta({ layout: "auth" });
const { t } = useI18n();
const api = useApi();
const email = ref("");
const sent = ref(false);
const error = ref<unknown>(null);

async function submit() {
  error.value = null;
  try {
    await api.post("/password-resets", { email: email.value }, { noReauth: true });
    sent.value = true;
  } catch (e) {
    error.value = e;
  }
}
</script>

<template>
  <Card>
    <CardHeader>
      <CardTitle>{{ t("reset.requestTitle") }}</CardTitle>
      <CardDescription>{{ sent ? t("reset.requestSent") : t("reset.requestDescription") }}</CardDescription>
    </CardHeader>
    <CardContent>
      <form v-if="!sent" class="grid gap-4" @submit.prevent="submit">
        <FormError :error="error" />
        <div class="grid gap-2">
          <Label for="email">{{ t("auth.email") }}</Label>
          <Input id="email" v-model="email" type="email" autocomplete="username" required />
        </div>
        <Button type="submit">{{ t("reset.send") }}</Button>
      </form>
      <NuxtLink to="/login" class="mt-4 block text-center text-sm text-muted-foreground hover:underline">{{ t("auth.backToLogin") }}</NuxtLink>
    </CardContent>
  </Card>
</template>
