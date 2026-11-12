<script setup lang="ts">
// ワークスペースの削除の確認（11-permissions-and-audit.md「ワークスペースと利用者」、INV-TEN-07・08）。サーバー管理者だけが使う。
// すぐに完全に消し、元に戻せない。ワークスペースの名前を打ち込んでもらい、本人の確認（再認証）をしてから消す
// （名前の照合と再認証の判断はサーバーが行う。再認証は useApi がダイアログを出す）。
import { TriangleAlert } from "@lucide/vue";

import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";

const props = defineProps<{ workspaceId: string; name: string }>();
const open = defineModel<boolean>("open", { required: true });
const emit = defineEmits<{ deleted: [] }>();
const { t } = useI18n();
const api = useApi();
const toast = useToast();

const typed = ref("");
const busy = ref(false);
const deleteError = ref<unknown>(null);
const matches = computed(() => typed.value.trim() === props.name);
watch(open, (v) => {
  if (v) {
    typed.value = "";
    deleteError.value = null;
  }
});

async function remove() {
  if (!matches.value) return;
  busy.value = true;
  deleteError.value = null;
  try {
    await api.del(`/workspaces/${props.workspaceId}`, {}, { confirmName: typed.value.trim() });
    open.value = false;
    toast.show(t("workspaceDelete.done", { name: props.name }));
    await refreshNuxtData("workspaces");
    emit("deleted");
  } catch (e) {
    deleteError.value = e;
  } finally {
    busy.value = false;
  }
}
</script>

<template>
  <Dialog v-model:open="open">
    <DialogContent class="overflow-hidden border-t-4 border-t-destructive" data-testid="workspace-delete-dialog">
      <form class="grid gap-4" @submit.prevent="remove">
        <DialogHeader>
          <DialogTitle class="flex items-center gap-2.5 font-mincho text-lg font-medium tracking-[0.06em]">
            <span class="flex size-9 shrink-0 items-center justify-center rounded-full bg-destructive/10 text-destructive"><TriangleAlert class="size-4.5" aria-hidden="true" /></span>
            {{ t("workspaceDelete.confirmTitle", { name }) }}
          </DialogTitle>
          <DialogDescription>{{ t("workspaceDelete.help") }}</DialogDescription>
        </DialogHeader>
        <div class="grid gap-2">
          <Label :for="`workspace-delete-name-${workspaceId}`">{{ t("workspaceDelete.typeName", { name }) }}</Label>
          <Input :id="`workspace-delete-name-${workspaceId}`" v-model="typed" autocomplete="off" data-testid="workspace-delete-name" />
        </div>
        <p class="text-xs text-muted-foreground">{{ t("workspaceDelete.reauthNote") }}</p>
        <FormError :error="deleteError" />
        <DialogFooter>
          <Button type="button" variant="outline" @click="open = false">{{ t("common.cancel") }}</Button>
          <Button type="submit" variant="destructive" :disabled="!matches || busy" data-testid="workspace-delete-confirm">{{ t("workspaceDelete.delete") }}</Button>
        </DialogFooter>
      </form>
    </DialogContent>
  </Dialog>
</template>
