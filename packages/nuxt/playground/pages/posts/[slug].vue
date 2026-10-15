<script setup lang="ts">
const route = useRoute();
const { data } = await useCmsQuery<{ postBySlug: { id: string; title: string; url: string; body: unknown } | null }>("postBySlug", {
  slug: String(route.params.slug),
  locale: "ja",
});
</script>

<template>
  <article v-if="data?.postBySlug">
    <h1>{{ data.postBySlug.title }}</h1>
    <CmsBody :doc="data.postBySlug.body as never" />
    <NuxtLink to="/">Back</NuxtLink>
  </article>
  <p v-else>Not found</p>
</template>
