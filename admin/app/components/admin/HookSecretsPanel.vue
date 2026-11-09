<script setup lang="ts">
// フックが読む秘密（04-hooks.md「秘密」）。登録した値は、この画面にも出さない。名前と更新日時だけを表示する。
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";

interface HookSecret {
  name: string;
  updatedAt: string;
}

const props = defineProps<{ workspaceId: string }>();
const { t, locale } = useI18n();
const api = useApi();
const base = computed(() => `/workspaces/${props.workspaceId}/hook-secrets`);
const { data: secrets, error, refresh } = useAsyncData(`hook-secrets:${props.workspaceId}`, () => api.get<HookSecret[]>(base.value));

const name = ref("");
const value = ref("");
const busy = ref(false);
const actionError = ref<unknown>(null);

async function save() {
  busy.value = true;
  actionError.value = null;
  try {
    await api.put(`${base.value}/${encodeURIComponent(name.value.trim())}`, { value: value.value });
    name.value = "";
    value.value = "";
    await refresh();
  } catch (e) {
    actionError.value = e;
  } finally {
    busy.value = false;
  }
}

async function remove(secret: HookSecret) {
  actionError.value = null;
  try {
    await api.del(`${base.value}/${encodeURIComponent(secret.name)}`);
    await refresh();
  } catch (e) {
    actionError.value = e;
  }
}
</script>

<template>
  <section class="grid gap-4" data-testid="hook-secrets">
    <p class="text-sm text-muted-foreground">{{ t("hookSecrets.help") }}</p>
    <FormError :error="error ?? actionError" />
    <form class="grid gap-3 sm:max-w-md" @submit.prevent="save">
      <div class="grid gap-2">
        <Label for="hook-secret-name">{{ t("hookSecrets.name") }}</Label>
        <Input id="hook-secret-name" v-model="name" autocomplete="off" required />
      </div>
      <div class="grid gap-2">
        <Label for="hook-secret-value">{{ t("hookSecrets.value") }}</Label>
        <Input id="hook-secret-value" v-model="value" type="password" autocomplete="new-password" required />
      </div>
      <div>
        <Button type="submit" :disabled="busy">{{ t("hookSecrets.save") }}</Button>
      </div>
    </form>
    <p v-if="secrets && secrets.length === 0" class="text-sm text-muted-foreground">{{ t("hookSecrets.empty") }}</p>
    <ul v-else class="grid gap-2">
      <li v-for="s in secrets" :key="s.name" class="flex flex-wrap items-center justify-between gap-2 rounded-md border p-3 text-sm">
        <span>
          <code class="break-all">{{ s.name }}</code>
          <span class="ml-2 text-muted-foreground">{{ t("hookSecrets.updatedAt", { at: formatDateTime(s.updatedAt, locale) }) }}</span>
        </span>
        <Button type="button" variant="outline" size="sm" @click="remove(s)">{{ t("hookSecrets.remove") }}</Button>
      </li>
    </ul>
  </section>
</template>
