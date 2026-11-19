<script setup lang="ts">
// 校正（AI。10-llm-and-budget.md「編集者向けのAI支援」の5、2026-11-17）。編集者がボタンを押したときだけ動く。
// - 保存済みの下書きを見出しのセクションごとに送り、「誤り」と「読みやすさ」の提案を並べる。押す前に、トークン数の上限の見積もりを出す。
// - 「当てる」は、元の語句が本文に1か所だけあり、書式を含まないときだけ出す（サーバーが保存済みの下書きで確かめ、
//   押したときに編集中の本文でも確かめる）。当てるのはエディタの操作なので、元に戻せて、自動保存で下書きに入る。
// - 提案の文は HTML にせず、文字のまま見せる。
import { Button } from "@/components/ui/button";
import type { ProofreadCheck, ProofreadSuggestion } from "~/utils/content";
import { applyFix } from "~/utils/proofread";
import AssistStateNote from "~/components/assist/AssistStateNote.vue";

const props = defineProps<{ workspaceId: string; entryId: string; flush?: () => Promise<unknown> }>();
const { t, locale } = useI18n();
const { result, error, start, load } = useAssistTask<ProofreadCheck>(() => `/workspaces/${props.workspaceId}/entries/${props.entryId}/assist/proofread`);
const editors = useBodyEditors();
onMounted(load);

const KINDS: ProofreadSuggestion["kind"][] = ["error", "style"];
const TITLES: Record<ProofreadSuggestion["kind"], string> = { error: "assist.fixError", style: "assist.fixStyle" };
/** 当てた提案と、当てられなかった提案（元の語句。この画面にいる間だけ覚える）。 */
const applied = ref(new Set<string>());
const failed = ref(new Set<string>());

const groups = computed(() =>
  KINDS.map((kind) => ({ kind, items: (result.value?.suggestions ?? []).filter((s) => s.kind === kind) })).filter((g) => g.items.length > 0),
);
const number = (n: number) => n.toLocaleString(locale.value);

/** 書きかけの下書きを保存してから始める（保存済みの下書きが対象）。 */
async function run() {
  applied.value = new Set();
  failed.value = new Set();
  await props.flush?.();
  await start();
}

function apply(s: ProofreadSuggestion) {
  const ok = applyFix(editors.value, s.from, s.to);
  const target = ok ? applied : failed;
  target.value = new Set([...target.value, s.from]);
}
</script>

<template>
  <section class="grid gap-3" data-testid="proofread">
    <h3 class="font-medium">{{ t("assist.proofreadTitle") }}</h3>
    <p class="text-muted-foreground">{{ t("assist.proofreadHelp") }}</p>
    <div class="flex flex-wrap items-center gap-3">
      <Button size="sm" variant="outline" :disabled="result?.state === 'running'" data-testid="proofread-start" @click="run">{{ t("assist.proofreadStart") }}</Button>
      <span v-if="result" class="text-xs text-muted-foreground" data-testid="proofread-estimate">
        {{ t("assist.estimate", { input: number(result.estimate.inputTokens), output: number(result.estimate.outputTokens) }) }}
      </span>
    </div>
    <p v-if="result?.state === 'disabled'" class="text-muted-foreground">{{ t("assist.disabled") }}</p>
    <AssistStateNote v-if="result" :state="result.state" :period="result.period" />
    <template v-if="result?.suggestions && result.state !== 'running'">
      <p v-if="result.state === 'done' && groups.length === 0" class="text-muted-foreground">{{ t("assist.noFixes") }}</p>
      <p v-if="result.state !== 'done' && groups.length > 0" class="text-xs text-muted-foreground">{{ t("assist.proofreadPartial") }}</p>
      <div v-for="g in groups" :key="g.kind" class="grid gap-2" :data-testid="`proofread-${g.kind}`">
        <h4 class="text-sm font-medium">{{ t(TITLES[g.kind]) }}</h4>
        <ul class="grid gap-2">
          <li v-for="s in g.items" :key="s.from" class="grid gap-1 rounded-md border p-2">
            <span class="text-muted-foreground line-through">{{ s.from }}</span>
            <span>{{ s.to }}</span>
            <span v-if="applied.has(s.from)" class="text-xs text-muted-foreground">{{ t("assist.fixApplied") }}</span>
            <span v-else-if="failed.has(s.from)" class="text-xs text-muted-foreground">{{ t("assist.fixApplyFailed") }}</span>
            <Button v-else-if="s.applicable" size="sm" variant="outline" class="w-fit" data-testid="proofread-apply" @click="apply(s)">{{ t("assist.fixApply") }}</Button>
            <span v-else class="text-xs text-muted-foreground">{{ t("assist.fixNotApplicable") }}</span>
          </li>
        </ul>
      </div>
      <p class="text-xs text-muted-foreground">{{ t("assist.proofreadNote") }}</p>
    </template>
    <FormError :error="error" />
  </section>
</template>
