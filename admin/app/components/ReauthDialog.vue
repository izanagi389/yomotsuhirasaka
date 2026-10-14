<script setup lang="ts">
// 再認証のダイアログ。多要素認証を設定した人は多要素認証（TOTP・復旧コード・パスキー）で、していない人はパスワードで行う（Q108）。
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";

const { t } = useI18n();
const { open, finish } = useReauth();
const { me, refresh } = useAuth();
const api = useApi();

const secret = ref("");
const useRecovery = ref(false);
const error = ref<unknown>(null);
const busy = ref(false);

watch(open, (v) => {
  if (v) {
    secret.value = "";
    error.value = null;
    useRecovery.value = false;
  }
});

async function submit() {
  busy.value = true;
  error.value = null;
  try {
    const body = !me.value?.mfaEnabled
      ? { password: secret.value }
      : useRecovery.value
        ? { recoveryCode: secret.value.trim() }
        : { totpCode: secret.value.trim() };
    await api.post("/session/reauth", body, { noReauth: true });
    await refresh();
    finish(true);
  } catch (e) {
    error.value = e;
  } finally {
    busy.value = false;
  }
}

async function withPasskey() {
  busy.value = true;
  error.value = null;
  try {
    const challenge = await api.post<{ publicKey: Record<string, unknown> }>("/session/reauth/passkey", undefined, { noReauth: true });
    const passkey = await usePasskey(challenge);
    await api.post("/session/reauth", { passkey }, { noReauth: true });
    await refresh();
    finish(true);
  } catch (e) {
    error.value = e;
  } finally {
    busy.value = false;
  }
}

function onOpenChange(v: boolean) {
  if (!v) finish(false);
}
</script>

<template>
  <Dialog :open="open" @update:open="onOpenChange">
    <DialogContent>
      <form class="grid gap-4" @submit.prevent="submit">
        <DialogHeader>
          <DialogTitle>{{ t("reauth.title") }}</DialogTitle>
          <DialogDescription>{{ t("reauth.description") }}</DialogDescription>
        </DialogHeader>
        <FormError :error="error" />
        <div class="grid gap-2">
          <Label for="reauth-secret">
            {{ !me?.mfaEnabled ? t("auth.password") : useRecovery ? t("auth.recoveryCode") : t("auth.totpCode") }}
          </Label>
          <Input
            id="reauth-secret"
            v-model="secret"
            :type="me?.mfaEnabled ? 'text' : 'password'"
            :autocomplete="me?.mfaEnabled ? 'one-time-code' : 'current-password'"
            :inputmode="me?.mfaEnabled && !useRecovery ? 'numeric' : undefined"
            required
          />
        </div>
        <div v-if="me?.mfaEnabled" class="flex flex-wrap gap-2 text-sm">
          <Button type="button" variant="link" class="h-auto p-0" @click="useRecovery = !useRecovery">
            {{ useRecovery ? t("auth.useTotp") : t("auth.useRecoveryCode") }}
          </Button>
          <Button type="button" variant="link" class="h-auto p-0" :disabled="busy" @click="withPasskey">{{ t("auth.usePasskey") }}</Button>
        </div>
        <DialogFooter>
          <Button type="button" variant="outline" @click="finish(false)">{{ t("common.cancel") }}</Button>
          <Button type="submit" :disabled="busy">{{ t("reauth.confirm") }}</Button>
        </DialogFooter>
      </form>
    </DialogContent>
  </Dialog>
</template>
