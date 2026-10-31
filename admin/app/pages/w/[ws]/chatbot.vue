<script setup lang="ts">
// チャットボットの回答の設定（09-qa-chatbot.md「語り口」、Q24, Q25。M4）。重い権限 manageLlm を持つ人とサーバー管理者が使う。
// - 語り口（丁寧さ・回答の長さ）。長さはLLMへの指示だけで、回答の長さの上限は変わらない
// - 自由記述のシステムプロンプト（上級者向け。既定は使わない。使うときは警告を出す）。使わない設定でも文は保存する
// - 答えられないときの文（コンテンツのロケールごと）
// - 編集者向けのAI支援を使うか（AssistSettingsPanel。すぐに保存する）
// 保存は変えた項目だけを送る。再認証・権限・値の検査はサーバーが行う（再認証のダイアログは useApi が出す）。
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
import type { AnswerSettings } from "~/utils/content";
import AssistSettingsPanel from "~/components/assist/AssistSettingsPanel.vue";

const MAX_FREE_PROMPT = 2000;
const MAX_PHRASE = 500;
const MAX_REFUSE_WORDS = 100;
const MAX_REFUSE_WORD = 100;

const route = useRoute();
const ws = route.params.ws as string;
const { t } = useI18n();
const api = useApi();
const { workspace, manifest, error: wsError } = await useWorkspace(ws);
const allowed = computed(() => hasPermission(workspace.value, "manageLlm"));
const { data, error } = await useAsyncData(`answer-settings:${ws}`, () =>
  allowed.value ? api.get<AnswerSettings>(`/workspaces/${ws}/answer-settings`) : Promise.resolve(null),
);

/** 編集中の値（読み込んだ値の写し）。 */
const form = ref<AnswerSettings | null>(null);
watch(data, (d) => (form.value = d ? structuredClone(toRaw(d)) : null), { immediate: true });

const chars = (s: string) => [...s].length;
/** 「情報なし」の文を、テンプレートのロケールの順（既定のロケールが先）に並べる。 */
const phraseLocales = computed(() => {
  const keys = Object.keys(form.value?.phrases ?? {});
  const order = manifest.value?.locales ?? [];
  const def = manifest.value?.defaultLocale;
  const rank = (l: string) => (l === def ? -1 : order.includes(l) ? order.indexOf(l) : order.length);
  return [...keys].sort((a, b) => rank(a) - rank(b) || a.localeCompare(b));
});
const freePromptTooLong = computed(() => !!form.value && chars(form.value.freePrompt.text) > MAX_FREE_PROMPT);
const phraseProblems = computed(() =>
  Object.entries(form.value?.phrases ?? {}).filter(([, p]) => p.noInformation.trim() === "" || chars(p.noInformation) > MAX_PHRASE).map(([l]) => l),
);
/** 拒否する語の入力（1行に1語）。 */
const refuseText = ref("");
watch(data, (d) => (refuseText.value = d ? d.refusal.words.join("\n") : ""), { immediate: true });
const refuseWords = computed(() => [...new Set(refuseText.value.split("\n").map((w) => w.trim()).filter((w) => w !== ""))]);
const refuseProblem = computed(() =>
  refuseWords.value.length > MAX_REFUSE_WORDS
    ? t("chatbot.refuseTooMany", { max: MAX_REFUSE_WORDS })
    : refuseWords.value.some((w) => chars(w) > MAX_REFUSE_WORD)
      ? t("chatbot.refuseTooLong", { max: MAX_REFUSE_WORD })
      : null,
);
const invalid = computed(
  () =>
    freePromptTooLong.value ||
    phraseProblems.value.length > 0 ||
    !!refuseProblem.value ||
    (!!form.value?.freePrompt.enabled && form.value.freePrompt.text.trim() === ""),
);

/** 読み込んだ値から変えた項目だけ。 */
function changes(): Partial<AnswerSettings> {
  const f = form.value!;
  const d = data.value!;
  const out: Partial<AnswerSettings> = {};
  if (f.politeness !== d.politeness) out.politeness = f.politeness;
  if (f.length !== d.length) out.length = f.length;
  const fp: Partial<AnswerSettings["freePrompt"]> = {};
  if (f.freePrompt.enabled !== d.freePrompt.enabled) fp.enabled = f.freePrompt.enabled;
  if (f.freePrompt.text !== d.freePrompt.text) fp.text = f.freePrompt.text;
  if (Object.keys(fp).length > 0) (out as Record<string, unknown>).freePrompt = fp;
  const phrases = Object.fromEntries(Object.entries(f.phrases).filter(([l, p]) => p.noInformation !== d.phrases[l]?.noInformation));
  if (Object.keys(phrases).length > 0) out.phrases = phrases;
  const refusal: Partial<AnswerSettings["refusal"]> = {};
  if (refuseWords.value.join("\n") !== d.refusal.words.join("\n")) refusal.words = refuseWords.value;
  if (f.refusal.creditCard !== d.refusal.creditCard) refusal.creditCard = f.refusal.creditCard;
  if (f.refusal.myNumber !== d.refusal.myNumber) refusal.myNumber = f.refusal.myNumber;
  if (Object.keys(refusal).length > 0) (out as Record<string, unknown>).refusal = refusal;
  return out;
}

const saving = ref(false);
const saveError = ref<unknown>(null);
const saved = ref(false);
async function save() {
  if (!form.value || invalid.value) return;
  const body = changes();
  if (Object.keys(body).length === 0) return;
  saving.value = true;
  saveError.value = null;
  saved.value = false;
  try {
    data.value = await api.put<AnswerSettings>(`/workspaces/${ws}/answer-settings`, body);
    saved.value = true;
  } catch (e) {
    saveError.value = e;
  } finally {
    saving.value = false;
  }
}

const politenessOptions = ["polite", "casual"] as const;
const lengthOptions = ["short", "standard", "detailed"] as const;
const capitalize = (s: string) => s.charAt(0).toUpperCase() + s.slice(1);
</script>

<template>
  <div class="grid gap-6">
    <WorkspaceNav :workspace-id="ws" :name="workspace?.name" :admin="isWorkspaceAdmin(workspace)" :budget="allowed" :questions="hasPermission(workspace, 'viewQuestionLog')" />
    <FormError :error="wsError ?? error ?? saveError" />
    <p v-if="workspace && !allowed" class="text-muted-foreground">{{ t("errors.forbidden") }}</p>

    <form v-else-if="form" class="grid gap-8 text-sm" data-testid="chatbot-settings" @submit.prevent="save">
      <div class="grid gap-2">
        <h2 class="text-lg font-semibold">{{ t("chatbot.title") }}</h2>
        <p class="text-muted-foreground">{{ t("chatbot.help") }}</p>
        <p class="text-muted-foreground" data-testid="chatbot-egress-note">{{ t("chatbot.egressNote") }}</p>
      </div>

      <!-- 語り口 -->
      <section class="grid gap-4">
        <h3 class="font-medium">{{ t("chatbot.toneTitle") }}</h3>
        <fieldset class="grid gap-2">
          <legend class="mb-1 font-medium">{{ t("chatbot.politeness") }}</legend>
          <label v-for="o in politenessOptions" :key="o" class="flex items-center gap-2">
            <input v-model="form.politeness" type="radio" name="politeness" :value="o" :disabled="saving" />
            {{ t(`chatbot.politeness${capitalize(o)}`) }}
          </label>
        </fieldset>
        <fieldset class="grid gap-2">
          <legend class="mb-1 font-medium">{{ t("chatbot.length") }}</legend>
          <label v-for="o in lengthOptions" :key="o" class="flex items-center gap-2">
            <input v-model="form.length" type="radio" name="length" :value="o" :disabled="saving" />
            {{ t(`chatbot.length${capitalize(o)}`) }}
          </label>
          <p class="text-muted-foreground">{{ t("chatbot.lengthHelp") }}</p>
        </fieldset>
      </section>

      <!-- 自由記述のシステムプロンプト -->
      <section class="grid gap-3 border-t pt-6" data-testid="chatbot-free-prompt">
        <h3 class="font-medium">{{ t("chatbot.freePromptTitle") }}</h3>
        <div class="flex items-center gap-2">
          <Switch id="free-prompt-enabled" v-model="form.freePrompt.enabled" :disabled="saving" />
          <Label for="free-prompt-enabled">{{ t("chatbot.freePromptUse") }}</Label>
        </div>
        <Alert v-if="form.freePrompt.enabled" variant="destructive" data-testid="chatbot-free-prompt-warning">
          <AlertDescription>{{ t("chatbot.freePromptWarning") }}</AlertDescription>
        </Alert>
        <div class="grid gap-1.5">
          <Label for="free-prompt">{{ t("chatbot.freePromptLabel", { max: MAX_FREE_PROMPT }) }}</Label>
          <Textarea id="free-prompt" v-model="form.freePrompt.text" rows="6" :disabled="saving" :aria-invalid="freePromptTooLong" />
          <p class="text-muted-foreground" :class="freePromptTooLong ? 'text-destructive' : ''">
            {{ t("chatbot.freePromptCount", { count: chars(form.freePrompt.text), max: MAX_FREE_PROMPT }) }}
          </p>
          <p v-if="!form.freePrompt.enabled" class="text-muted-foreground">{{ t("chatbot.freePromptOff") }}</p>
        </div>
      </section>

      <!-- 答えられないときの文 -->
      <section class="grid gap-3 border-t pt-6" data-testid="chatbot-phrases">
        <h3 class="font-medium">{{ t("chatbot.phrasesTitle") }}</h3>
        <p class="text-muted-foreground">{{ t("chatbot.phrasesHelp", { max: MAX_PHRASE }) }}</p>
        <div v-for="locale in phraseLocales" :key="locale" class="grid gap-1.5">
          <Label :for="`phrase-${locale}`">{{ locale }}</Label>
          <Textarea :id="`phrase-${locale}`" v-model="form.phrases[locale]!.noInformation" rows="2" :disabled="saving" :aria-invalid="phraseProblems.includes(locale)" />
        </div>
      </section>

      <!-- 答えない質問（拒否する語と組み込みの種類） -->
      <section class="grid gap-3 border-t pt-6" data-testid="chatbot-refusal">
        <h3 class="font-medium">{{ t("chatbot.refusalTitle") }}</h3>
        <p class="text-muted-foreground">{{ t("chatbot.refusalHelp") }}</p>
        <div class="grid gap-1.5">
          <Label for="refuse-words">{{ t("chatbot.refuseWords", { max: MAX_REFUSE_WORDS }) }}</Label>
          <Textarea id="refuse-words" v-model="refuseText" rows="5" :disabled="saving" :aria-invalid="!!refuseProblem" />
          <p class="text-muted-foreground">{{ t("chatbot.refuseWordsHelp", { count: refuseWords.length }) }}</p>
          <p v-if="refuseProblem" class="text-destructive" role="alert">{{ refuseProblem }}</p>
        </div>
        <label class="flex items-center gap-2">
          <input v-model="form.refusal.creditCard" type="checkbox" :disabled="saving" />
          {{ t("chatbot.refuseCreditCard") }}
        </label>
        <label class="flex items-center gap-2">
          <input v-model="form.refusal.myNumber" type="checkbox" :disabled="saving" />
          {{ t("chatbot.refuseMyNumber") }}
        </label>
      </section>

      <div class="flex items-center gap-3">
        <Button type="submit" :disabled="saving || invalid">{{ t("common.save") }}</Button>
        <span v-if="saved" class="text-muted-foreground" role="status">{{ t("chatbot.saved") }}</span>
      </div>
    </form>
    <div v-if="allowed" class="border-t pt-6">
      <AssistSettingsPanel :workspace-id="ws" />
    </div>
  </div>
</template>
