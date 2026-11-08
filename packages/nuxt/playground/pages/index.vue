<script setup lang="ts">
const { data } = await useYomiQuery<{ postList: { id: string; title: string; slug: string }[] }>("postList", { locale: "ja" });
</script>

<template>
  <main>
    <h1>Posts</h1>
    <ul>
      <li v-for="p in data?.postList ?? []" :key="p.id">
        <NuxtLink :to="`/posts/${p.slug}`">{{ p.title }}</NuxtLink>
      </li>
    </ul>
  </main>
</template>
