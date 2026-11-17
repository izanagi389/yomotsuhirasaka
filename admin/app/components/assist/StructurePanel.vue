<script setup lang="ts">
// 構成の確認（AI。10-llm-and-budget.md「編集者向けのAI支援」の4、2026-11-17）。編集者がボタンを押したときだけ動く。
// - 保存済みの下書きについて、論理の流れ・答えられなかった質問・近い記事との重なりを、指摘として並べる（当てるものはない）。
// - 答えられなかった質問は、質問ログの閲覧の権限と再認証がある人だけに含まれる（サーバーが決める）。
// - 押す前に、トークン数の上限の見積もりを出す。指摘の文は HTML にせず、文字のまま見せる。
import { Button } from "@/components/ui/button";
import type { StructureCheck, StructureFinding } from "~/utils/content";
import AssistStateNote from "~/components/assist/AssistStateNote.vue";

const props = defineProps<{ workspaceId: string; entryId: string; flush?: () => Promise<unknown> }>();
const { t, locale } = useI18n();
const { result, error, start, load } = useAssistTask<StructureCheck>(() => `/workspaces/${props.workspaceId}/entries/${props.entryId}/assist/structure`);
const reauth = useReauth();
onMounted(load);

const KINDS: StructureFinding["kind"][] = ["flow", "question", "overlap"];
const TITLES: Record<StructureFinding["kind"], string> = {
  flow: "assist.findingFlow",
  question: "assist.findingQuestion",
  overlap: "assist.findingOverlap",
};
const groups = computed(() =>
  KINDS.map((kind) => ({ kind, items: (result.value?.suggestions ?? []).filter((f) => f.kind === kind) })).filter((g) => g.items.length > 0),
);
const number = (n: number) => n.toLocaleString(locale.value);

/** 書きかけの下書きを保存してから始める（保存済みの下書きが対象）。 */
async function run() {
  await props.flush?.();
  await start();
}

async function reauthenticate() {
  if (await reauth.request()) await load();
}
</script>

<template>
  <section class="grid gap-3" data-testid="structure-check">
    <h3 class="font-medium">{{ t("assist.structureTitle") }}</h3>
    <p class="text-muted-foreground">{{ t("assist.structureHelp") }}</p>
    <template v-if="result">
      <p v-if="result.questions === 'included'" class="text-muted-foreground">{{ t("assist.questionsIncluded") }}</p>
      <p v-else-if="result.questions === 'noPermission'" class="text-muted-foreground">{{ t("assist.questionsNoPermission") }}</p>
      <p v-else-if="result.questions === 'logDisabled'" class="text-muted-foreground">{{ t("assist.questionsLogDisabled") }}</p>
      <p v-else class="flex flex-wrap items-center gap-2 text-muted-foreground">
        {{ t("assist.questionsReauth") }}
        <Button size="sm" variant="link" class="h-auto p-0" data-testid="structure-reauth" @click="reauthenticate">{{ t("reauth.title") }}</Button>
      </p>
    </template>
    <div class="flex flex-wrap items-center gap-3">
      <Button size="sm" variant="outline" :disabled="result?.state === 'running'" data-testid="structure-start" @click="run">{{ t("assist.structureStart") }}</Button>
      <span v-if="result" class="text-xs text-muted-foreground" data-testid="structure-estimate">
        {{ t("assist.estimate", { input: number(result.estimate.inputTokens), output: number(result.estimate.outputTokens) }) }}
      </span>
    </div>
    <p v-if="result?.state === 'disabled'" class="text-muted-foreground">{{ t("assist.disabled") }}</p>
    <AssistStateNote v-if="result" :state="result.state" :period="result.period" />
    <template v-if="result?.state === 'done'">
      <p v-if="groups.length === 0" class="text-muted-foreground">{{ t("assist.noFindings") }}</p>
      <div v-for="g in groups" :key="g.kind" class="grid gap-2" :data-testid="`structure-${g.kind}`">
        <h4 class="text-sm font-medium">{{ t(TITLES[g.kind]) }}</h4>
        <ul class="grid gap-2">
          <li v-for="(f, i) in g.items" :key="i" class="grid gap-1 rounded-md border p-2">
            <span v-if="f.question" class="text-xs text-muted-foreground">「{{ f.question }}」</span>
            <NuxtLink
              v-if="f.entryId"
              :to="`/w/${workspaceId}/entries/${f.entryId}`"
              class="w-fit text-xs underline underline-offset-2"
            >
              {{ f.entryTitle || t("assist.otherEntryUntitled") }}
            </NuxtLink>
            <span :class="f.text ? '' : 'text-muted-foreground'">{{ f.text || t("assist.explanationWithheld") }}</span>
          </li>
        </ul>
      </div>
      <p class="text-xs text-muted-foreground">{{ t("assist.structureNote") }}</p>
    </template>
    <FormError :error="error" />
  </section>
</template>
