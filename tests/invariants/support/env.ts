// テスト環境の接続先。テスト用のDocker Composeが環境変数で渡す（README）。

function required(name: string): string {
  const value = process.env[name];
  if (!value) {
    throw new Error(`環境変数 ${name} が設定されていません（tests/invariants/README.md の「テスト環境」）`);
  }
  return value;
}

export const env = {
  /** 起動済みのCMS（本番モード・HTTPS）の接続先。 */
  get cmsUrl(): string {
    return required("CMS_TEST_URL");
  },
  /** テスト環境の準備で作ったサーバー管理者。 */
  get serverAdmin() {
    return {
      email: required("CMS_TEST_SERVER_ADMIN_EMAIL"),
      password: required("CMS_TEST_SERVER_ADMIN_PASSWORD"),
      totpSecret: required("CMS_TEST_SERVER_ADMIN_TOTP_SECRET"),
    };
  },
  /** CMSが普段の処理に使うのと同じDBの接続先（INV-DB-* 用）。 */
  get dbAppUrl(): string {
    return required("CMS_TEST_DB_APP_URL");
  },
  /** DBのスーパーユーザーの接続先。テーブルの一覧を調べるためだけに使う。 */
  get dbSuperuserUrl(): string {
    return required("CMS_TEST_DB_SUPERUSER_URL");
  },
  /** PostgreSQL 15 の接続先（16より古いDBで起動しないことの確認用）。 */
  get pg15Url(): string {
    return required("CMS_TEST_PG15_URL");
  },
  /** PostgreSQL 15 の、移行・ロールの作成用の接続先（CREATEROLE を持ち、スーパーユーザーでないロール）。 */
  get pg15MigrationUrl(): string {
    return required("CMS_TEST_PG15_MIGRATION_URL");
  },
  /**
   * テストの実行環境のホスト名（CMSから見た名前）。CMSの外部への送信の例外として許可しておく。
   * 受信サーバー・擬似IdP・擬似HIBP・出口のプロキシはこのホストで待ち受ける。
   */
  get harnessAllowedHost(): string {
    return required("CMS_TEST_HARNESS_ALLOWED_HOST");
  },
  /** テストの実行環境の、例外として許可していない内部のアドレス（IPアドレス）。 */
  get harnessBlockedHost(): string {
    return required("CMS_TEST_HARNESS_BLOCKED_HOST");
  },
  /** harnessAllowedHost のIPアドレス。 */
  get harnessAllowedIp(): string {
    return required("CMS_TEST_HARNESS_ALLOWED_IP");
  },
  /**
   * テストの実行環境で動かすDNSサーバー（support/dns.ts）のUDPのポート。
   * テスト環境では、CMSが `inv.test` のゾーンをこのDNSサーバーに問い合わせるようにしておく。
   */
  get harnessDnsPort(): number {
    return Number(required("CMS_TEST_HARNESS_DNS_PORT"));
  },
  /** `*.inv.test` と harnessAllowedHost に有効なTLSの証明書と鍵（CMSはこの証明書のCAを信頼しておく）。 */
  get harnessTls(): { certFile: string; keyFile: string } {
    return { certFile: required("CMS_TEST_HARNESS_TLS_CERT"), keyFile: required("CMS_TEST_HARNESS_TLS_KEY") };
  },
  /** 擬似IdP（support/oidc-idp.ts）。テスト環境のCMSにはこの発行者をOIDCの接続先として設定しておく。 */
  get oidc(): { port: number; issuer: string; clientId: string; clientSecret: string; jitDomain: string } {
    return {
      port: Number(required("CMS_TEST_OIDC_PORT")),
      issuer: required("CMS_TEST_OIDC_ISSUER"),
      clientId: required("CMS_TEST_OIDC_CLIENT_ID"),
      clientSecret: required("CMS_TEST_OIDC_CLIENT_SECRET"),
      jitDomain: required("CMS_TEST_OIDC_JIT_DOMAIN"),
    };
  },
  /** 擬似LLM（support/mock-llm.ts）のOpenAI互換のAPIの接続先（`…/v1`）。 */
  get mockLlmUrl(): string {
    return required("CMS_TEST_MOCK_LLM_URL");
  },
  /** CMSのメディアの保存先を含む、ファイルシステムの上位のディレクトリ（テストの実行環境からも見える）。 */
  get fsRoot(): string {
    return required("CMS_TEST_FS_ROOT");
  },
};
