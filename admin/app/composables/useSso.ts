// 外部のOIDC/SSO（13-security.md「OIDC/SSO」）。IdPへはページごと移動し、戻り先は /sso/complete。
//
// - IdPへ移動する前に、戻った後に開く画面（この管理画面の中のパス）を sessionStorage に覚えておく。
// - IdPから戻ったら、サーバーが /admin/sso/complete?purpose=…[&error=…] へ転送する（pages/sso/complete.vue）。

const RETURN_KEY = "yomi:sso:return";

export interface SsoInfo {
  enabled: boolean;
  displayName?: string;
}

/** この管理画面の中のパスだけを戻り先にする（別のサイトへの転送に使わせない）。 */
export function safeReturnPath(path: unknown): string | null {
  return typeof path === "string" && path.startsWith("/") && !path.startsWith("//") && !path.startsWith("/\\") ? path : null;
}

export function takeSsoReturn(): string | null {
  try {
    const v = sessionStorage.getItem(RETURN_KEY);
    sessionStorage.removeItem(RETURN_KEY);
    return safeReturnPath(v);
  } catch {
    return null;
  }
}

export function useSso() {
  const info = useState<SsoInfo | null>("sso:info", () => null);
  const api = useApi();

  async function load(): Promise<SsoInfo> {
    if (!info.value) {
      try {
        info.value = await api.get<SsoInfo>("/session/oidc", { noReauth: true });
      } catch {
        info.value = { enabled: false };
      }
    }
    return info.value;
  }

  /** SSOを始め、IdPへ移動する。`path` は開始のAPI、`returnTo` は戻った後に開く画面。 */
  async function start(path: string, body: unknown, returnTo: string | null): Promise<void> {
    const res = await api.post<{ authorizationUrl: string }>(path, body, { noReauth: path === "/session/oidc/start" });
    try {
      if (returnTo) sessionStorage.setItem(RETURN_KEY, returnTo);
      else sessionStorage.removeItem(RETURN_KEY);
    } catch {
      // 覚えられなければ、戻った後はホームを開く
    }
    window.location.assign(res.authorizationUrl);
  }

  return { info, load, start };
}
