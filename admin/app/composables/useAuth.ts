// ログイン中の本人の情報（GET /api/admin/me）。権限はサーバーがリクエストのたびに確かめるので、ここでは画面の出し分けにだけ使う。

export interface Me {
  id: string;
  email: string;
  isServerAdmin: boolean;
  mfaEnabled: boolean;
  /** 多要素認証の設定が済むまで、他の操作ができない（Q197）。 */
  mfaSetupRequired: boolean;
  locale: "ja" | "en" | null;
  effectiveLocale: "ja" | "en";
  reauthSecondsLeft: number;
  /** 外部のOIDC/SSOでログインしたセッションか。 */
  sso: boolean;
  /** 管理操作の前に、CMS側のパスキーの登録が要るか（SSOの管理者。Q131）。 */
  passkeyRequired: boolean;
  /** IdPでの再ログインで再認証できるか（SSOの、管理者でない人）。 */
  idpReauthAllowed: boolean;
  /** 本人が決めた表示名（未設定なら null。決めるまで、プロフィールの画面で入力を求める）。 */
  displayName: string | null;
  /** 表示名の入力欄の初期値（SSOのIdPが返した名前）。 */
  displayNameSuggestion: string | null;
}

export function useAuth() {
  const me = useState<Me | null>("auth:me", () => null);
  const loaded = useState<boolean>("auth:loaded", () => false);

  async function refresh(): Promise<Me | null> {
    try {
      me.value = await useApi().get<Me>("/me", { noReauth: true });
    } catch (e) {
      if (e instanceof ApiError && (e.status === 401 || e.status === 403)) me.value = null;
      else throw e;
    } finally {
      loaded.value = true;
    }
    return me.value;
  }

  function forget(): void {
    me.value = null;
    setCsrfToken(null);
  }

  async function logout(): Promise<void> {
    try {
      await useApi().del("/session", { noReauth: true });
    } finally {
      forget();
      await navigateTo("/login");
    }
  }

  return { me, loaded, refresh, forget, logout };
}
