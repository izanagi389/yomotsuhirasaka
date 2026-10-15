// ログインしていなければログインの画面へ。多要素認証の設定が要る人は、設定の画面へ（Q197）。
// 権限の判断はサーバーがリクエストのたびに行う。ここは画面の行き先を決めるだけ。

const PUBLIC = new Set(["/login", "/forgot-password", "/reset-password", "/reset-mfa", "/invitations/accept"]);

export default defineNuxtRouteMiddleware(async (to) => {
  const { me, loaded, refresh } = useAuth();
  if (!loaded.value) await refresh();
  if (PUBLIC.has(to.path)) return;
  if (!me.value) return navigateTo({ path: "/login", query: to.fullPath !== "/" ? { next: to.fullPath } : {} });
  if (me.value.mfaSetupRequired && to.path !== "/account/security") return navigateTo("/account/security");
});
