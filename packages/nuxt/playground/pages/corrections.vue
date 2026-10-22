<script setup lang="ts">
const { data } = await useCmsCorrections({ locale: "ja", limit: 100 });
</script>

<template>
  <main>
    <h1>訂正の一覧</h1>
    <p><a href="/corrections.xml">RSS</a> / <a href="/corrections.json">JSON Feed</a></p>
    <CmsCorrections :items="data?.corrections" />
  </main>
</template>
