<script setup lang="ts">
// サイト内検索の確かめ（部品と、自分で描く場合）。
const { text, result, status } = useYomiSearch({ locale: "ja", first: 5 });
</script>

<template>
  <main>
    <h1>Search</h1>
    <YomiSearchBox label="サイト内を検索" locale="ja">
      <template #loading>検索しています…</template>
      <template #empty>見つかりませんでした。</template>
      <template #rateLimited>しばらくしてからもう一度お試しください。</template>
      <template #error>検索できませんでした。</template>
    </YomiSearchBox>

    <h2>useYomiSearch</h2>
    <input v-model="text" type="search" aria-label="自分で描く検索欄" />
    <p>{{ status }}</p>
    <ol v-if="result">
      <li v-for="hit in result.hits" :key="hit.entryId">
        <a :href="`${hit.url}#${hit.anchor}`">{{ hit.breadcrumb.join(" › ") || hit.url }}</a>
        <p>
          <template v-for="(s, i) in yomiHighlight(hit.excerpt, hit.matches)" :key="i"><mark v-if="s.match">{{ s.text }}</mark><template v-else>{{ s.text }}</template></template>
        </p>
      </li>
    </ol>
  </main>
</template>
