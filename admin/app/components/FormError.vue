<script setup lang="ts">
// APIのエラーを、翻訳ファイルの文言で表示する（コードに対応する文言がなければ、一般的な文言）。
import { Alert, AlertDescription } from "@/components/ui/alert";

const props = defineProps<{ error: unknown }>();
const { t, te } = useI18n();

const message = computed(() => {
  const e = props.error;
  if (!e) return null;
  if (e instanceof ApiError) {
    const key = `errors.${e.code}`;
    return te(key) ? t(key) : t("errors.generic", { status: e.status });
  }
  if (e instanceof Error && te(`errors.${e.message}`)) return t(`errors.${e.message}`);
  return t("errors.network");
});
</script>

<template>
  <Alert v-if="message" variant="destructive" role="alert">
    <AlertDescription>{{ message }}</AlertDescription>
  </Alert>
</template>
