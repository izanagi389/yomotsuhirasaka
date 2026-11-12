<script setup lang="ts">
// ワークスペースの削除（11-permissions-and-audit.md「ワークスペースと利用者」、INV-TEN-07・08）。サーバー管理者だけが使う。
// すぐに完全に消し、元に戻せない。ワークスペースの名前を打ち込んでもらい、本人の確認（再認証）をしてから消す
// （名前の照合と再認証の判断はサーバーが行う。再認証は useApi がダイアログを出す）。
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";

const props = defineProps<{ workspaceId: string; name: string }>();
const { t } = useI18n();
const api = useApi();
const typed = ref("");
const confirming = ref(false);
const busy = ref(false);
const deleteError = ref<unknown>(null);
const matches = computed(() => typed.value.trim() === props.name);
async function remove() {
  busy.value = true;
  deleteError.value = null;
  try {
    await api.del(`/workspaces/${props.workspaceId}`, {}, { confirmName: typed.value.trim() });
    confirming.value = false;
    await refreshNuxtData("workspaces");
    await navigateTo("/");
  } catch (e) {
    deleteError.value = e;
  } finally {
    busy.value = false;
  }
}
</script>

<template>
  <section class="grid gap-3 rounded-md border border-destructive/50 p-4 text-sm" data-testid="workspace-delete">
    <h2 class="text-lg font-semibold text-destructive">{{ t("workspaceDelete.title") }}</h2>
    <p class="text-muted-foreground">{{ t("workspaceDelete.help") }}</p>
    <div class="grid max-w-md gap-2">
      <Label for="workspace-delete-name">{{ t("workspaceDelete.typeName", { name }) }}</Label>
      <Input id="workspace-delete-name" v-model="typed" autocomplete="off" />
    </div>
    <div>
      <Button variant="destructive" :disabled="!matches" data-testid="workspace-delete-open" @click="confirming = true">{{ t("workspaceDelete.delete") }}</Button>
    </div>
    <ConfirmDialog
      v-model:open="confirming"
      :title="t('workspaceDelete.confirmTitle', { name })"
      :description="t('workspaceDelete.confirmDescription')"
      :confirm-label="t('workspaceDelete.delete')"
      destructive
      :busy="busy"
      :error="deleteError"
      @confirm="remove"
    />
  </section>
</template>
