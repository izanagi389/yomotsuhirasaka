<script setup lang="ts">
// ワークスペースの中のサイドバー。上でワークスペースを切り替え、真ん中に画面の一覧、下に本人のメニューを置く。
// 管理は管理者とサーバー管理者だけ、予算とチャットボットは重い権限 manageLlm を持つ人とサーバー管理者だけ、
// 質問ログは viewQuestionLog を持つ人だけに出す（権限の判断はサーバー）。
// 「やること」には、その人が対応できる項目の件数を出す（0件なら出さない）。
import { Bot, ChartColumn, ChevronsUpDown, CircleCheckBig, FileText, Image, LayoutGrid, MessagesSquare, Settings } from "@lucide/vue";

import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import type { WorkspaceInfo } from "~/utils/content";

const props = defineProps<{ workspaceId: string }>();
const { t } = useI18n();
const route = useRoute();
const api = useApi();

const { data: workspace } = useAsyncData(`ws-nav:${props.workspaceId}`, () => api.get<WorkspaceInfo>(`/workspaces/${props.workspaceId}`));
const { data: workspaces } = useAsyncData("workspaces", () => api.get<{ id: string; name: string }[]>("/workspaces"));
const { data: todo } = useTodo(props.workspaceId);

const base = computed(() => `/w/${props.workspaceId}`);
const main = computed(() => [
  { to: base.value, label: t("nav.todo"), icon: CircleCheckBig, count: todo.value?.length ?? 0, exact: true },
  { to: `${base.value}/entries`, label: t("nav.entries"), icon: FileText },
  { to: `${base.value}/media`, label: t("nav.media"), icon: Image },
  ...(hasPermission(workspace.value, "manageLlm")
    ? [
        { to: `${base.value}/chatbot`, label: t("nav.chatbot"), icon: Bot },
        { to: `${base.value}/budget`, label: t("nav.budget"), icon: ChartColumn },
      ]
    : []),
  ...(hasPermission(workspace.value, "viewQuestionLog") ? [{ to: `${base.value}/questions`, label: t("nav.questions"), icon: MessagesSquare }] : []),
]);
const admin = computed(() => (isWorkspaceAdmin(workspace.value) ? [{ to: `${base.value}/admin`, label: t("nav.admin"), icon: Settings }] : []));

function active(to: string, exact?: boolean): boolean {
  return exact ? route.path === to : route.path === to || route.path.startsWith(`${to}/`);
}
</script>

<template>
  <div class="flex h-full flex-col gap-1 bg-sidebar px-3 py-4 text-sm text-sidebar-foreground" data-testid="workspace-sidebar">
    <NuxtLink to="/" class="flex items-center gap-2.5 rounded-md px-2 pt-1 pb-3 text-sidebar-strong">
      <BrandMark :size="28" />
      <span class="font-mincho text-base tracking-[0.18em]">{{ t("app.name") }}</span>
    </NuxtLink>
    <Mizuhiki class="mb-3" />

    <DropdownMenu>
      <DropdownMenuTrigger
        class="mb-3 flex min-h-11 w-full items-center gap-2.5 rounded-[10px] border border-sidebar-border bg-white/5 p-2 text-left text-sidebar-strong outline-none hover:bg-sidebar-active focus-visible:ring-2 focus-visible:ring-ring"
        :aria-label="t('workspaceSwitcher.open')"
        data-testid="workspace-switcher"
      >
        <Kamon :id="workspaceId" />
        <span class="min-w-0 flex-1 truncate font-medium">{{ workspace?.name }}</span>
        <ChevronsUpDown class="size-4 text-sidebar-muted" aria-hidden="true" />
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" class="w-60">
        <DropdownMenuLabel class="text-xs font-normal text-muted-foreground">{{ t("workspaceSwitcher.label") }}</DropdownMenuLabel>
        <DropdownMenuItem v-for="w in workspaces ?? []" :key="w.id" @select="navigateTo(`/w/${w.id}`)">
          <Kamon :id="w.id" :size="20" />
          <span class="truncate" :class="w.id === workspaceId ? 'font-medium' : ''">{{ w.name }}</span>
        </DropdownMenuItem>
        <DropdownMenuSeparator />
        <DropdownMenuItem @select="navigateTo('/')"><LayoutGrid aria-hidden="true" />{{ t("workspaceSwitcher.all") }}</DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>

    <nav :aria-label="workspace?.name ?? t('nav.label')" class="grid gap-0.5">
      <SidebarLink v-for="i in main" :key="i.to" :to="i.to" :label="i.label" :icon="i.icon" :count="'count' in i ? i.count : 0" :active="active(i.to, 'exact' in i && i.exact)" />
      <template v-if="admin.length">
        <span v-if="admin.length > 1" class="px-2.5 pt-4 pb-1 text-[11px] tracking-[0.2em] text-sidebar-muted">{{ t("nav.adminSection") }}</span>
        <span v-else aria-hidden="true" class="mx-2.5 my-2 h-px bg-sidebar-border" />
        <SidebarLink v-for="i in admin" :key="i.to" :to="i.to" :label="i.label" :icon="i.icon" :active="active(i.to)" />
      </template>
    </nav>

    <div class="mt-auto pt-4">
      <AccountMenu placement="sidebar" />
    </div>
  </div>
</template>
