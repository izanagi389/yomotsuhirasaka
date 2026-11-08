<script setup lang="ts">
import type { YomiDeletedAnchor } from "@yomotsuhirasaka/nuxt";

const route = useRoute();
const { data } = await useYomiQuery<{ postBySlug: { id: string; title: string; url: string; body: unknown; deletedAnchors: YomiDeletedAnchor[]; aiTrainingOptOut: boolean } | null }>("postBySlug", {
  slug: String(route.params.slug),
  locale: "ja",
});
useYomiAiMeta(() => data.value?.postBySlug);
</script>

<template>
  <article v-if="data?.postBySlug">
    <h1>{{ data.postBySlug.title }}</h1>
    <YomiDeletedAnchorNotice :anchors="data.postBySlug.deletedAnchors" />
    <YomiBody :doc="data.postBySlug.body as never" />
    <NuxtLink to="/">Back</NuxtLink>
  </article>
  <p v-else>Not found</p>
</template>
