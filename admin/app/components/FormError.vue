<script setup lang="ts">
// APIのエラーを、翻訳ファイルの文言で表示する（コードに対応する文言がなければ、一般的な文言）。
import { Alert, AlertDescription } from "@/components/ui/alert";

const props = defineProps<{ error: unknown }>();
const { t, te } = useI18n();

// 入力の問題は、どこが悪いかをサーバーの文（英語）でそのまま添える
const DETAILED = new Set(["invalid_entry", "destination_not_allowed"]);

// useAsyncData のエラーは Nuxt のエラーに包まれて届く。元の ApiError は cause にあるので取り出す
// （取り出さないと、テンプレートが未適用などの理由まで「サーバーにつながりません。」と出てしまう）
const unwrapped = computed<unknown>(() => {
  const e = props.error;
  if (e && typeof e === "object" && !(e instanceof ApiError) && "cause" in e && e.cause instanceof ApiError) return e.cause;
  return e;
});
// 多要素認証の設定が要る操作では、設定の画面への道を示す
const needsMfa = computed(() => unwrapped.value instanceof ApiError && (unwrapped.value.code === "mfa_required" || unwrapped.value.code === "mfa_setup_required"));
const detail = computed(() => (unwrapped.value instanceof ApiError && DETAILED.has(unwrapped.value.code) ? unwrapped.value.message : null));

const message = computed(() => {
  const e = unwrapped.value;
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
    <AlertDescription>
      {{ message }}
      <span v-if="detail" class="mt-1 block break-words text-xs opacity-80">{{ detail }}</span>
      <NuxtLink v-if="needsMfa" to="/account/security" class="mt-1 block font-medium underline" data-testid="mfa-setup-link">{{ t("common.setUpMfa") }}</NuxtLink>
    </AlertDescription>
  </Alert>
</template>
