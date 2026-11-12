<script setup lang="ts">
// 自分の表示名。レビューする人の候補や、レビューの履歴の「誰が」に、メールアドレスの代わりに出る。
// 決めるまでは、管理画面のほかの画面に進めない（middleware/auth.global.ts）。変えられるのは本人だけ。
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";

const { t } = useI18n();
const api = useApi();
const route = useRoute();
const { me, refresh } = useAuth();
const required = computed(() => !me.value?.displayName);
const name = ref(me.value?.displayName ?? me.value?.displayNameSuggestion ?? "");
const saving = ref(false);
const error = ref<unknown>(null);
const saved = ref(false);

/** 戻り先（同じ管理画面の中のパスだけ）。 */
function nextPath(): string {
  const next = route.query["next"];
  return typeof next === "string" && next.startsWith("/") && !next.startsWith("//") ? next : "/";
}

async function save() {
  saving.value = true;
  error.value = null;
  saved.value = false;
  const wasRequired = required.value;
  try {
    await api.patch("/me", { displayName: name.value });
    await refresh();
    if (wasRequired) await navigateTo(nextPath());
    else saved.value = true;
  } catch (e) {
    error.value = e;
  } finally {
    saving.value = false;
  }
}
</script>

<template>
  <div class="grid max-w-3xl gap-6 text-sm">
    <PageHeader :title="t('account.nav.profile')" :description="t('security.title')" />
    <AccountNav />
    <p v-if="required" class="rounded-md border border-amber-500/50 bg-amber-500/10 p-3" role="status" data-testid="display-name-required">
      {{ t("account.profile.required") }}
    </p>
    <form class="grid max-w-md gap-2" @submit.prevent="save">
      <Label for="display-name">{{ t("account.profile.displayName") }}</Label>
      <Input id="display-name" v-model="name" maxlength="50" required autocomplete="name" data-testid="display-name" />
      <p class="text-muted-foreground">{{ t("account.profile.help") }}</p>
      <FormError :error="error" />
      <p v-if="saved" role="status">{{ t("account.profile.saved") }}</p>
      <Button type="submit" class="w-fit" :disabled="saving || !name.trim()" data-testid="save-display-name">{{ t("common.save") }}</Button>
    </form>
  </div>
</template>
