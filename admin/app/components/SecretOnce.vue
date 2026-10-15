<script setup lang="ts">
// 一度だけ表示する秘密の値（APIキー・Webhook の署名の秘密・招待やリセットのリンクなど）。
// サーバーは秘密の値を保存しない（ハッシュか暗号化）ので、この表示を閉じると二度と見られない。
import { Check, Copy } from "@lucide/vue";

import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";

defineProps<{ value: string; label: string }>();
const emit = defineEmits<{ dismiss: [] }>();
const { t } = useI18n();
const copied = ref(false);

async function copy(value: string) {
  try {
    await navigator.clipboard.writeText(value);
    copied.value = true;
    setTimeout(() => (copied.value = false), 2000);
  } catch {
    copied.value = false;
  }
}
</script>

<template>
  <Alert role="status" data-testid="secret-once">
    <AlertTitle>{{ label }}</AlertTitle>
    <AlertDescription class="grid gap-2">
      <p>{{ t("secretOnce.warning") }}</p>
      <div class="flex items-center gap-2">
        <code class="min-w-0 flex-1 break-all rounded bg-muted px-2 py-1 font-mono text-xs text-foreground" data-testid="secret-value">{{ value }}</code>
        <Button type="button" size="sm" variant="outline" :aria-label="t('secretOnce.copy')" @click="copy(value)">
          <component :is="copied ? Check : Copy" class="size-4" />
        </Button>
      </div>
      <Button type="button" size="sm" variant="ghost" class="w-fit" @click="emit('dismiss')">{{ t("secretOnce.dismiss") }}</Button>
    </AlertDescription>
  </Alert>
</template>
