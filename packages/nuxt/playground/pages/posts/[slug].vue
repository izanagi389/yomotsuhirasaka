<script setup lang="ts">
import type { CmsDeletedAnchor } from "@new-cms/nuxt";

const route = useRoute();
const { data } = await useCmsQuery<{ postBySlug: { id: string; title: string; url: string; body: unknown; deletedAnchors: CmsDeletedAnchor[]; aiTrainingOptOut: boolean } | null }>("postBySlug", {
  slug: String(route.params.slug),
  locale: "ja",
});
useCmsAiMeta(() => data.value?.postBySlug);
</script>

<template>
  <article v-if="data?.postBySlug">
    <h1>{{ data.postBySlug.title }}</h1>
    <CmsDeletedAnchorNotice :anchors="data.postBySlug.deletedAnchors" />
    <CmsBody :doc="data.postBySlug.body as never" />
    <NuxtLink to="/">Back</NuxtLink>
  </article>
  <p v-else>Not found</p>
</template>
