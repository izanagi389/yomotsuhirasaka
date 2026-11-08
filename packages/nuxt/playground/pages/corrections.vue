<script setup lang="ts">
const { data } = await useYomiCorrections({ locale: "ja", limit: 100 });
</script>

<template>
  <main>
    <h1>訂正の一覧</h1>
    <p><a href="/corrections.xml">RSS</a> / <a href="/corrections.json">JSON Feed</a></p>
    <YomiCorrections :items="data?.corrections" />
  </main>
</template>
