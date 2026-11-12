<script setup lang="ts">
// useConfirm() の確認のダイアログ。ほかのダイアログ（メディアの詳細など）から開くことがあるので、その上に重ねる
// （再認証のダイアログはさらにその上）。
import { TriangleAlert } from "@lucide/vue";

import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";

const { t } = useI18n();
const { state, settle } = useConfirm();
const open = computed({
  get: () => state.value.open,
  set: (v: boolean) => {
    if (!v) settle(false);
  },
});
</script>

<template>
  <Dialog v-model:open="open">
    <DialogContent
      class="z-[65]"
      overlay-class="z-[65]"
      :class="state.destructive ? 'border-t-4 border-t-destructive' : ''"
      data-testid="confirm-dialog"
    >
      <DialogHeader>
        <DialogTitle class="flex items-center gap-2.5 font-mincho text-lg font-medium tracking-[0.04em]">
          <span v-if="state.destructive" class="flex size-8 shrink-0 items-center justify-center rounded-full bg-destructive/10 text-destructive">
            <TriangleAlert class="size-4" aria-hidden="true" />
          </span>
          {{ state.title }}
        </DialogTitle>
        <DialogDescription>{{ state.description }}</DialogDescription>
      </DialogHeader>
      <DialogFooter>
        <Button variant="outline" @click="settle(false)">{{ t("common.cancel") }}</Button>
        <Button :variant="state.destructive ? 'destructive' : 'default'" data-testid="confirm" @click="settle(true)">{{ state.confirmLabel }}</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
