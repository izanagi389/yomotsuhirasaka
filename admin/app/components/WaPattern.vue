<script setup lang="ts">
// 和柄（麻の葉・七宝・青海波）を、親の要素いっぱいに薄く敷く。色は currentColor、濃さは親で opacity を決める。
// 飾りなので読み上げない。和柄はログイン画面と、何もないときの画面だけに使う（持ち主の判断）。
defineProps<{ kind: "asanoha" | "shippo" | "seigaiha" }>();
const id = useId();
</script>

<template>
  <svg aria-hidden="true" class="pointer-events-none absolute inset-0 size-full">
    <defs>
      <pattern v-if="kind === 'asanoha'" :id="id" width="40" height="69.3" patternUnits="userSpaceOnUse">
        <path
          d="M0 0L40 0L20 34.6ZM0 0L20 11.5M40 0L20 11.5M20 34.6L20 11.5M0 0L20 34.6L-20 34.6ZM0 0L0 23.1M20 34.6L0 23.1M-20 34.6L0 23.1M40 0L60 34.6L20 34.6ZM40 0L40 23.1M60 34.6L40 23.1M20 34.6L40 23.1M20 34.6L0 69.3L40 69.3ZM20 34.6L20 57.7M0 69.3L20 57.7M40 69.3L20 57.7M-20 34.6L20 34.6L0 69.3ZM-20 34.6L0 46.2M20 34.6L0 46.2M0 69.3L0 46.2M20 34.6L60 34.6L40 69.3ZM20 34.6L40 46.2M60 34.6L40 46.2M40 69.3L40 46.2"
          fill="none"
          stroke="currentColor"
          stroke-width="0.8"
        />
      </pattern>
      <pattern v-else-if="kind === 'shippo'" :id="id" width="32" height="32" patternUnits="userSpaceOnUse">
        <g fill="none" stroke="currentColor" stroke-width="1">
          <circle cx="0" cy="0" r="16" /><circle cx="32" cy="0" r="16" /><circle cx="0" cy="32" r="16" /><circle cx="32" cy="32" r="16" /><circle cx="16" cy="16" r="16" />
        </g>
      </pattern>
      <pattern v-else :id="id" width="36" height="18" patternUnits="userSpaceOnUse">
        <g fill="none" stroke="currentColor" stroke-width="0.8">
          <circle cx="18" cy="18" r="17" /><circle cx="18" cy="18" r="12" /><circle cx="18" cy="18" r="7" />
          <circle cx="0" cy="9" r="17" /><circle cx="0" cy="9" r="12" /><circle cx="0" cy="9" r="7" />
          <circle cx="36" cy="9" r="17" /><circle cx="36" cy="9" r="12" /><circle cx="36" cy="9" r="7" />
        </g>
      </pattern>
    </defs>
    <rect width="100%" height="100%" :fill="`url(#${id})`" />
  </svg>
</template>
