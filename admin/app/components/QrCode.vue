<script setup lang="ts">
// Draws a QR code entirely in the browser. The value (e.g. an otpauth:// URL holding a TOTP secret)
// never leaves this page: no external QR service, no image request, no v-html.
import { computed } from "vue";
import { encode } from "uqr";

const props = defineProps<{ value: string; label: string }>();

// The quiet zone of 4 modules is what the QR specification asks for; scanners read it more reliably.
const BORDER = 4;

const qr = computed(() => {
  const { size, data } = encode(props.value, { ecc: "M", border: BORDER });
  let path = "";
  data.forEach((row, y) => {
    row.forEach((dark, x) => {
      if (dark) path += `M${x} ${y}h1v1h-1z`;
    });
  });
  return { size, path };
});
</script>

<template>
  <svg
    role="img"
    :aria-label="label"
    :viewBox="`0 0 ${qr.size} ${qr.size}`"
    shape-rendering="crispEdges"
    class="size-48 rounded"
  >
    <!-- Always dark on white, also in dark mode: many scanners cannot read inverted codes -->
    <rect :width="qr.size" :height="qr.size" fill="#ffffff" />
    <path :d="qr.path" fill="#000000" />
  </svg>
</template>
