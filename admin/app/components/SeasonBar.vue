<script setup lang="ts">
// 季節の帯（二十四節気・七十二候・その日の日付）。日本語の画面では和暦、英語の画面では訳と西暦で出す。
// variant="panel" はログイン画面の左側の大きな表示。
import { seasonOf, wareki } from "~/utils/season";

withDefaults(defineProps<{ variant?: "bar" | "panel" }>(), { variant: "bar" });
const { locale } = useI18n();

// 日付は開いた時点で決める（日をまたいで開いたままなら、1時間ごとに見直す）
const now = ref(new Date());
let timer: ReturnType<typeof setInterval> | undefined;
onMounted(() => {
  timer = setInterval(() => (now.value = new Date()), 60 * 60 * 1000);
});
onBeforeUnmount(() => clearInterval(timer));

const season = computed(() => seasonOf(now.value));
const ja = computed(() => locale.value === "ja");
const dateText = computed(() =>
  ja.value
    ? wareki(now.value)
    : new Intl.DateTimeFormat("en", { dateStyle: "long", timeZone: "Asia/Tokyo" }).format(now.value),
);
</script>

<template>
  <div v-if="variant === 'bar'" class="flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-muted-foreground" data-testid="season-bar">
    <span aria-hidden="true" class="flex gap-0.5">
      <span v-for="c in season.colors" :key="c" class="size-2.5 rounded-[2px]" :style="{ backgroundColor: c }" />
    </span>
    <span class="font-mincho tracking-[0.1em] text-foreground">{{ ja ? season.sekki.ja : season.sekki.en }}</span>
    <span>{{ ja ? `${season.kou.ja}（${season.kou.yomi}）` : season.kou.en }}</span>
    <span class="ml-auto">{{ dateText }}</span>
  </div>
  <div v-else class="grid gap-3" data-testid="season-panel">
    <span class="font-mincho text-3xl tracking-[0.2em]">{{ ja ? season.sekki.ja : season.sekki.en }}</span>
    <span class="text-[15px] text-sidebar-foreground">{{ ja ? `${season.kou.ja} ― ${season.kou.yomi}` : season.kou.en }}</span>
    <span class="text-xs text-sidebar-muted">{{ dateText }}</span>
  </div>
</template>
