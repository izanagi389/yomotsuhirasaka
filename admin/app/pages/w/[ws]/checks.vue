<script setup lang="ts">
// 前の「要対応」画面の URL。「やること」（ワークスペースの入り口）へ移す。
const route = useRoute();
await navigateTo(`/w/${route.params.ws as string}`, { replace: true });
</script>

<template>
  <div />
</template>
