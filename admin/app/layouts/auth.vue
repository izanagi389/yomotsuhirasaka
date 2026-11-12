<script setup lang="ts">
// ログイン・パスワードの再設定・招待の受け入れなどの画面。左に藍の地と麻の葉・水引・その日の季節、右にフォームを置く
// （狭い画面では上下に並ぶ）。
const { t } = useI18n();
</script>

<template>
  <div class="flex min-h-svh flex-wrap bg-background">
    <div class="relative flex min-h-56 flex-[1_1_420px] flex-col justify-between gap-8 overflow-hidden bg-sidebar p-8 text-sidebar-strong md:p-12">
      <WaPattern kind="asanoha" class="text-white opacity-10" />
      <div class="relative flex items-center gap-3">
        <BrandMark :size="36" />
        <span class="font-mincho text-lg tracking-[0.2em]">{{ t("app.name") }}</span>
      </div>
      <div class="relative grid max-w-md gap-4 max-md:hidden">
        <Mizuhiki class="w-56" />
        <SeasonBar variant="panel" />
      </div>
    </div>
    <main class="flex flex-[1_1_420px] items-center justify-center px-4 py-12">
      <div class="w-full max-w-sm">
        <slot />
      </div>
    </main>
  </div>
</template>
