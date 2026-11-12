// ワークスペースの紋（形と色）。まだ選んでいないワークスペースは、IDから決める。

export const KAMON_MOTIFS = [
  "shippo",
  "asanoha",
  "hishi",
  "sakura",
  "ume",
  "kikyo",
  "fuji",
  "take",
  "nami",
  "kumo",
  "ogi",
  "kuyo",
] as const;
export type KamonMotif = (typeof KAMON_MOTIFS)[number];

/** 藍・浅葱・朱・山吹・青磁・鼠。地の色（bg）と線の色（fg）。 */
export const KAMON_COLORS = {
  ai: { bg: "#dce5f1", fg: "#1e3f66" },
  asagi: { bg: "#d6eef1", fg: "#1f6e7c" },
  shu: { bg: "#f8e1da", fg: "#a23b26" },
  yamabuki: { bg: "#f6e7c1", fg: "#7a5410" },
  seiji: { bg: "#dceee7", fg: "#1d6b5e" },
  nezumi: { bg: "#e6e8eb", fg: "#3b4757" },
} as const;
export type KamonColor = keyof typeof KAMON_COLORS;
const COLOR_KEYS = Object.keys(KAMON_COLORS) as KamonColor[];

/** FNV-1a（32ビット）。見た目の割り当てに使うだけで、安全のためのものではない。 */
function hash(s: string): number {
  let h = 0x811c9dc5;
  for (let i = 0; i < s.length; i++) {
    h ^= s.charCodeAt(i);
    h = Math.imul(h, 0x01000193) >>> 0;
  }
  return h;
}

export function kamonFor(id: string): { motif: KamonMotif; color: (typeof KAMON_COLORS)[KamonColor] } {
  const h = hash(id);
  return {
    motif: KAMON_MOTIFS[h % KAMON_MOTIFS.length]!,
    color: KAMON_COLORS[COLOR_KEYS[(h >>> 8) % COLOR_KEYS.length]!],
  };
}
