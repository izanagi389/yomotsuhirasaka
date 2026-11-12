<script setup lang="ts">
// プレビュー（03-content-and-editing.md「プレビュー（Q72）」、計画の13番）。記事の画面の「プレビュー」タブ。
// - サイトで開くURLは「公開URLのオリジン ＋ 設定のプレビュー用のパス ＋ ?token=<プレビュー用トークン>」。公開URLのオリジンが
//   ワークスペースの「プレビューを許すサイト」になければ開かない（CSP の frame-src も同じ一覧。server/src/http/admin_ui.rs）。
// - iframe には sandbox を付ける。サイトのスクリプト・自分のオリジンのクッキー・フォーム・新しいウィンドウは許し、
//   管理画面そのものを別のページへ移すこと（top navigation）などは許さない。
// - ライブプレビュー: タブを開いている間、iframe でサイトを開く。サイトから `ready` を受け取ったら（そのサイトのオリジンと
//   iframe の窓からだけ。INV-PRV-03、@yomotsuhirasaka/preview）、自動保存のたびに中身のない `refresh` を送る（宛先のオリジンを指定）。
// - トークンは10分で切れるので、8分ごとに作り直して開き直す。タブを閉じる・画面を離れるときに失効させる。
// - CSP の frame-src は記事の画面を直接開いたときだけ付くので、画面の中の移動で来たときは、保存してから読み直す。
// - 「別タブで開く」は、別のトークンで新しいタブを開く（失効は有効期間に任せる）。
import { READY, REFRESH, acceptPreviewMessage } from "@yomotsuhirasaka/preview";
import { Button } from "@/components/ui/button";
import type { PreviewToken, WorkspaceInfo } from "~/utils/content";

const props = defineProps<{
  workspaceId: string;
  entryId: string;
  workspace: WorkspaceInfo | null | undefined;
  /** 保存済みのリビジョン（変わったら、サイトに読み直しを知らせる）。 */
  revision: number;
  /** 未保存の変更を保存する。 */
  flush: () => Promise<boolean>;
}>();
const { t } = useI18n();
const api = useApi();
const base = computed(() => `/workspaces/${props.workspaceId}/entries/${props.entryId}/preview-tokens`);
const initialPath = useState<string>("initial-path");

const RENEW_MS = 8 * 60 * 1000;
const current = ref<PreviewToken | null>(null);
const src = ref("");
const siteOrigin = ref("");
const problem = ref<"noUrl" | "notAllowed" | null>(null);
const error = ref<unknown>(null);
const ready = ref(false);
const frame = ref<HTMLIFrameElement | null>(null);
let renewTimer: ReturnType<typeof setTimeout> | undefined;

/** トークンのURLのオリジンが許されていれば、開くURL。 */
function previewUrl(token: PreviewToken): string | null {
  if (!token.url) {
    problem.value = "noUrl";
    return null;
  }
  const origin = new URL(token.url).origin;
  if (!(props.workspace?.previewOrigins ?? []).includes(origin)) {
    problem.value = "notAllowed";
    return null;
  }
  problem.value = null;
  siteOrigin.value = origin;
  const u = new URL(props.workspace?.previewPath ?? "/_yomi/preview", origin);
  u.searchParams.set("token", token.token);
  return u.toString();
}

function revoke(token: PreviewToken | null) {
  if (!token) return;
  const csrf = csrfToken();
  void fetch(`/api/admin${base.value}/${token.id}`, {
    method: "DELETE",
    keepalive: true,
    credentials: "same-origin",
    headers: csrf ? { "X-CSRF-Token": csrf } : {},
  }).catch(() => {});
}

async function open() {
  clearTimeout(renewTimer);
  error.value = null;
  try {
    await props.flush();
    const token = await api.post<PreviewToken>(base.value, {});
    const url = previewUrl(token);
    const old = current.value;
    current.value = token;
    revoke(old);
    if (!url) {
      revoke(token);
      current.value = null;
      src.value = "";
      return;
    }
    ready.value = false;
    src.value = url;
    renewTimer = setTimeout(open, RENEW_MS);
  } catch (e) {
    error.value = e;
  }
}

function onMessage(event: MessageEvent) {
  const message = acceptPreviewMessage(event, { origins: siteOrigin.value ? [siteOrigin.value] : [], source: frame.value?.contentWindow });
  if (message?.type === READY) ready.value = true;
}

// 保存のたびに、サイトに読み直しを知らせる（中身は載せない。サイトがCMSから取り直す）
watch(
  () => props.revision,
  () => {
    if (ready.value && siteOrigin.value) frame.value?.contentWindow?.postMessage({ type: REFRESH }, siteOrigin.value);
  },
);

async function openInNewTab() {
  error.value = null;
  // トークンを待つ間にポップアップを止められないよう、先に空のタブを開く
  const w = window.open("about:blank", "_blank");
  try {
    await props.flush();
    const token = await api.post<PreviewToken>(base.value, {});
    const url = previewUrl(token);
    if (!url || !w) {
      w?.close();
      revoke(token);
      return;
    }
    w.opener = null;
    w.location.href = url;
  } catch (e) {
    w?.close();
    error.value = e;
  }
}

const onPageHide = () => revoke(current.value);
onMounted(async () => {
  // 画面の中の移動で来たときは、CSP に frame-src がないので、保存してから記事の画面を読み直す
  if (initialPath.value !== window.location.pathname) {
    await props.flush();
    const u = new URL(window.location.href);
    u.searchParams.set("tab", "preview");
    window.location.assign(u.toString());
    return;
  }
  window.addEventListener("message", onMessage);
  window.addEventListener("pagehide", onPageHide);
  await open();
});
onBeforeUnmount(() => {
  clearTimeout(renewTimer);
  window.removeEventListener("message", onMessage);
  window.removeEventListener("pagehide", onPageHide);
  revoke(current.value);
  current.value = null;
});
</script>

<template>
  <div class="grid gap-3 text-sm" data-testid="preview-panel">
    <FormError :error="error" />
    <p v-if="problem === 'noUrl'" class="text-muted-foreground">{{ t("preview.noUrl") }}</p>
    <p v-else-if="problem === 'notAllowed'" class="text-muted-foreground" data-testid="preview-not-allowed">
      {{ t("preview.notAllowed") }}
      <NuxtLink v-if="isWorkspaceAdmin(workspace)" :to="`/w/${workspaceId}/admin/settings`" class="underline">{{ t("preview.openSettings") }}</NuxtLink>
    </p>
    <div class="flex flex-wrap items-center gap-2">
      <Button size="sm" variant="outline" data-testid="preview-new-tab" @click="openInNewTab">{{ t("preview.openInNewTab") }}</Button>
      <Button v-if="src" size="sm" variant="ghost" @click="open">{{ t("preview.reload") }}</Button>
      <span v-if="src" class="text-xs text-muted-foreground">{{ ready ? t("preview.live") : t("preview.connecting") }}</span>
    </div>
    <iframe
      v-if="src"
      ref="frame"
      :src="src"
      :title="t('preview.frameTitle')"
      class="h-[75svh] w-full rounded-md border bg-white"
      referrerpolicy="no-referrer"
      sandbox="allow-scripts allow-same-origin allow-forms allow-popups"
      data-testid="preview-frame"
    />
    <p v-if="src" class="text-xs text-muted-foreground">{{ t("preview.cookieNote") }}</p>
  </div>
</template>
