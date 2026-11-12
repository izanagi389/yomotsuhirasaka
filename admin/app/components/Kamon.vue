<script setup lang="ts">
// ワークスペースの紋。管理者が選んだもの（crest）を出し、選んでいなければ12の形と6の色をワークスペースのIDから決める。
// 形は伝統の文様をもとに、この管理画面用に線で描き起こしたもの。飾りなので読み上げない。
import { kamonFor, type Crest } from "~/utils/kamon";

const props = withDefaults(defineProps<{ id: string; crest?: Crest | null; size?: number }>(), { size: 28, crest: null });
const k = computed(() => kamonFor(props.id, props.crest));
</script>

<template>
  <svg aria-hidden="true" :width="size" :height="size" viewBox="0 0 28 28" class="shrink-0">
    <circle cx="14" cy="14" r="13.5" :fill="k.color.bg" />
    <g fill="none" :stroke="k.color.fg" stroke-width="1.15" stroke-linejoin="round" stroke-linecap="round">
      <template v-if="k.motif === 'shippo'">
        <circle cx="14" cy="10" r="4" /><circle cx="10" cy="14" r="4" /><circle cx="18" cy="14" r="4" /><circle cx="14" cy="18" r="4" />
      </template>
      <path v-else-if="k.motif === 'asanoha'" d="M14 5L21.8 9.5V18.5L14 23L6.2 18.5V9.5ZM14 5V23M6.2 9.5L21.8 18.5M21.8 9.5L6.2 18.5" />
      <path v-else-if="k.motif === 'hishi'" d="M14 5L23 14L14 23L5 14ZM14 9.5L18.5 14L14 18.5L9.5 14Z" />
      <template v-else-if="k.motif === 'sakura'">
        <ellipse v-for="a in [0, 72, 144, 216, 288]" :key="a" cx="14" cy="8.6" rx="2.9" ry="4" :transform="`rotate(${a} 14 14)`" />
        <circle cx="14" cy="14" r="1.2" />
      </template>
      <template v-else-if="k.motif === 'ume'">
        <circle v-for="a in [0, 72, 144, 216, 288]" :key="a" cx="14" cy="9" r="3.4" :transform="`rotate(${a} 14 14)`" />
        <circle cx="14" cy="14" r="1.6" />
      </template>
      <path v-else-if="k.motif === 'kikyo'" d="M14 4.5l2.6 6.4h6.8l-5.5 4 2.1 6.6-6-4.2-6 4.2 2.1-6.6-5.5-4h6.8z" />
      <template v-else-if="k.motif === 'fuji'">
        <path d="M6 6.5h16" />
        <ellipse cx="9" cy="12" rx="2.2" ry="3.4" /><ellipse cx="14" cy="14" rx="2.2" ry="4.6" /><ellipse cx="19" cy="12" rx="2.2" ry="3.4" />
      </template>
      <path v-else-if="k.motif === 'take'" d="M11 5V23M17 5V23M9.5 10H12.5M15.5 13.5H18.5M9.5 18H12.5M17 9L21 6.5M11 15L7 12.5" />
      <path v-else-if="k.motif === 'nami'" d="M5 17C8 12 11 12 14 17S20 22 23 17M5 11.5C8 6.5 11 6.5 14 11.5S20 16.5 23 11.5" />
      <path v-else-if="k.motif === 'kumo'" d="M8.5 18.5A3.6 3.6 0 0 1 9 11.3A5.1 5.1 0 0 1 18.6 10.6A3.9 3.9 0 0 1 19.6 18.5Z" />
      <path v-else-if="k.motif === 'ogi'" d="M14 22L5.5 10.5A12 12 0 0 1 22.5 10.5ZM14 22L10 7.4M14 22L18 7.4M14 22V6.5" />
      <template v-else>
        <circle cx="14" cy="14" r="3.2" />
        <circle v-for="a in [0, 45, 90, 135, 180, 225, 270, 315]" :key="a" cx="14" cy="6.8" r="1.7" :transform="`rotate(${a} 14 14)`" />
      </template>
    </g>
  </svg>
</template>
