<script setup lang="ts">
// ワークスペースの管理（管理者とサーバー管理者）: 所属者・招待・配信APIのキー・Webhook・訂正の定型文・監査ログ。
// 権限の判断はサーバーが行う。発行や変更の多くは再認証が要り、サーバーが求めたらダイアログで確認する。
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import ApiKeysPanel from "~/components/admin/ApiKeysPanel.vue";
import AuditPanel from "~/components/admin/AuditPanel.vue";
import AuditSinksPanel from "~/components/admin/AuditSinksPanel.vue";
import CorrectionTemplatesPanel from "~/components/admin/CorrectionTemplatesPanel.vue";
import InvitationsPanel from "~/components/admin/InvitationsPanel.vue";
import MembersPanel from "~/components/admin/MembersPanel.vue";
import RelatedRecomputePanel from "~/components/admin/RelatedRecomputePanel.vue";
import WebhooksPanel from "~/components/admin/WebhooksPanel.vue";
import WorkspaceSettingsPanel from "~/components/admin/WorkspaceSettingsPanel.vue";
import type { MemberInfo } from "~/utils/admin";

const route = useRoute();
const ws = route.params.ws as string;
const { t } = useI18n();
const api = useApi();
const { workspace, manifest, error } = await useWorkspace(ws);
const tab = ref("members");

// 監査ログに、操作した人のメールアドレスを出す（所属者の一覧から分かる人だけ）
const people = ref<Record<string, string>>({});
watch(tab, async (v) => {
  if (v !== "audit" || Object.keys(people.value).length > 0) return;
  try {
    const members = await api.get<MemberInfo[]>(`/workspaces/${ws}/members`);
    people.value = Object.fromEntries(members.map((m) => [m.userId, m.email]));
  } catch {
    // 分からなければIDのまま
  }
});
</script>

<template>
  <div class="grid gap-6">
    <WorkspaceNav :workspace-id="ws" :name="workspace?.name" :admin="isWorkspaceAdmin(workspace)" :budget="hasPermission(workspace, 'manageLlm')" />
    <FormError :error="error" />
    <p v-if="workspace && !isWorkspaceAdmin(workspace)" class="text-muted-foreground">{{ t("errors.forbidden") }}</p>
    <Tabs v-else-if="workspace" v-model="tab">
      <TabsList class="flex-wrap">
        <TabsTrigger value="members">{{ t("adminTabs.members") }}</TabsTrigger>
        <TabsTrigger value="invitations">{{ t("adminTabs.invitations") }}</TabsTrigger>
        <TabsTrigger value="apiKeys">{{ t("adminTabs.apiKeys") }}</TabsTrigger>
        <TabsTrigger value="webhooks">{{ t("adminTabs.webhooks") }}</TabsTrigger>
        <TabsTrigger value="correctionTemplates">{{ t("adminTabs.correctionTemplates") }}</TabsTrigger>
        <TabsTrigger value="audit">{{ t("adminTabs.audit") }}</TabsTrigger>
        <TabsTrigger value="settings">{{ t("adminTabs.settings") }}</TabsTrigger>
      </TabsList>
      <TabsContent value="members" class="mt-4"><MembersPanel :workspace-id="ws" /></TabsContent>
      <TabsContent value="settings" class="mt-4 grid gap-8">
        <WorkspaceSettingsPanel :workspace-id="ws" />
        <RelatedRecomputePanel :workspace-id="ws" />
      </TabsContent>
      <TabsContent value="invitations" class="mt-4"><InvitationsPanel :workspace-id="ws" /></TabsContent>
      <TabsContent value="apiKeys" class="mt-4"><ApiKeysPanel :workspace-id="ws" /></TabsContent>
      <TabsContent value="webhooks" class="mt-4"><WebhooksPanel :workspace-id="ws" /></TabsContent>
      <TabsContent value="correctionTemplates" class="mt-4">
        <CorrectionTemplatesPanel :workspace-id="ws" :locales="manifest?.locales ?? []" />
      </TabsContent>
      <TabsContent value="audit" class="mt-4 grid gap-8">
        <AuditPanel :workspace-id="ws" :people="people" />
        <AuditSinksPanel :workspace-id="ws" />
      </TabsContent>
    </Tabs>
  </div>
</template>
