<script setup lang="ts">
// 質問ログと「書くべき記事の候補」（09-qa-chatbot.md「質問ログ」、Q14, Q20, Q76）。重い権限 viewQuestionLog を持つ人が使う
// （管理者にも既定では付かない）。見るには再認証が要り、見るたびに監査ログに残る（サーバー）。
// - 候補: 答えられなかった質問を同じ質問ごとにまとめたもの（件数の多い順）。質問文はマスクしたもの。消せる
// - 直近の日ごとの件数（答えた・情報なし・拒否・エラー）
// - 候補から、書くべき記事の構成案を作れる（AI支援。M4。OutlinePanel）
// 質問文は文字として出す（v-html は使わない）。
import { Button } from "@/components/ui/button";
import type { QuestionLogView } from "~/utils/content";
import OutlinePanel from "~/components/assist/OutlinePanel.vue";

const route = useRoute();
const ws = route.params.ws as string;
const { t, locale } = useI18n();
const api = useApi();
const { workspace, error: wsError } = await useWorkspace(ws);
const allowed = computed(() => hasPermission(workspace.value, "viewQuestionLog"));
const { data, error, refresh } = await useAsyncData(`question-log:${ws}`, () =>
  allowed.value ? api.get<QuestionLogView>(`/workspaces/${ws}/question-log`) : Promise.resolve(null),
);

const deleting = ref<QuestionLogView["candidates"][number] | null>(null);
const busy = ref(false);
const deleteError = ref<unknown>(null);
async function remove() {
  if (!deleting.value) return;
  busy.value = true;
  deleteError.value = null;
  try {
    await api.del(`/workspaces/${ws}/question-log/candidates/${deleting.value.id}`);
    deleting.value = null;
    await refresh();
  } catch (e) {
    deleteError.value = e;
  } finally {
    busy.value = false;
  }
}
</script>

<template>
  <div class="grid gap-6">
    <WorkspaceNav :workspace-id="ws" :name="workspace?.name" :admin="isWorkspaceAdmin(workspace)" :budget="hasPermission(workspace, 'manageLlm')" :questions="allowed" />
    <FormError :error="wsError ?? error" />
    <p v-if="workspace && !allowed" class="text-muted-foreground">{{ t("errors.forbidden") }}</p>

    <template v-else-if="data">
      <section class="grid gap-3 text-sm" data-testid="question-candidates">
        <h2 class="text-lg font-semibold">{{ t("questionLog.candidatesTitle") }}</h2>
        <p class="text-muted-foreground">{{ t("questionLog.candidatesHelp", { days: data.retentionDays }) }}</p>
        <p v-if="!data.enabled" class="text-muted-foreground" data-testid="question-log-disabled">{{ t("questionLog.disabled") }}</p>
        <p v-else-if="data.candidates.length === 0" class="text-muted-foreground">{{ t("questionLog.noCandidates") }}</p>
        <ul v-else class="grid gap-2">
          <li v-for="c in data.candidates" :key="c.id" class="flex flex-wrap items-start gap-3 rounded-md border p-3">
            <span class="min-w-0 flex-1 whitespace-pre-wrap break-words">{{ c.question }}</span>
            <span class="text-muted-foreground">{{ c.locale }}</span>
            <span class="tabular-nums">{{ t("questionLog.count", { count: c.count }) }}</span>
            <span class="text-muted-foreground">{{ formatDateTime(c.lastAskedAt, locale) }}</span>
            <Button variant="outline" size="sm" @click="deleting = c">{{ t("questionLog.delete") }}</Button>
            <OutlinePanel :workspace-id="ws" :candidate-id="c.id" />
          </li>
        </ul>
      </section>

      <section class="grid gap-3 border-t pt-6 text-sm" data-testid="question-daily">
        <h2 class="text-lg font-semibold">{{ t("questionLog.dailyTitle") }}</h2>
        <p v-if="data.daily.length === 0" class="text-muted-foreground">{{ t("questionLog.noDaily") }}</p>
        <table v-else class="w-full max-w-2xl text-left">
          <thead class="text-muted-foreground">
            <tr>
              <th class="py-1 font-medium">{{ t("questionLog.date") }}</th>
              <th class="py-1 font-medium">{{ t("questionLog.answered") }}</th>
              <th class="py-1 font-medium">{{ t("questionLog.noInformation") }}</th>
              <th class="py-1 font-medium">{{ t("questionLog.refused") }}</th>
              <th class="py-1 font-medium">{{ t("questionLog.error") }}</th>
            </tr>
          </thead>
          <tbody class="tabular-nums">
            <tr v-for="d in data.daily" :key="d.date" class="border-t">
              <td class="py-1">{{ d.date }}</td>
              <td class="py-1">{{ d.answered }}</td>
              <td class="py-1">{{ d.noInformation }}</td>
              <td class="py-1">{{ d.refused }}</td>
              <td class="py-1">{{ d.error }}</td>
            </tr>
          </tbody>
        </table>
      </section>
    </template>

    <ConfirmDialog
      :open="deleting !== null"
      :title="t('questionLog.deleteTitle')"
      :description="t('questionLog.deleteDescription')"
      :confirm-label="t('questionLog.delete')"
      destructive
      :busy="busy"
      :error="deleteError"
      @update:open="(v) => !v && (deleting = null)"
      @confirm="remove"
    />
  </div>
</template>
