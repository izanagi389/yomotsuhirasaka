// テストの舞台: 2つのワークスペース（A・B）と、それぞれの利用者・公開記事・APIキー。
// テストのファイルごとに新しく作るので、他のファイルの結果に左右されない。

import { randomUUID } from "node:crypto";
import { asBearer, driver, type CmsClient, type Credentials, type IssuedSecret, type Member, type OperationTarget, type RegisteredQuery, type ServerConfig, type Session } from "./driver.js";
import { SEARCHABLE_TEMPLATE } from "./template.js";
import { simpleBody } from "./body.js";
import { env } from "./env.js";
import { mockLlm } from "./mock-llm.js";

/** 応答に含まれてはいけない値を見分けるための、一意な目印。 */
export const marker = (label: string): string => `MARKER-${label}-${randomUUID()}`;

/** テストごとに一意なメールアドレス。 */
export const uniqueEmail = (label: string): string => `${label.toLowerCase()}-${randomUUID().slice(0, 12)}@inv.test`;

let ipCounter = 0;
/** テストごとに一意な接続元のIPアドレス（198.18.0.0/15、ベンチマーク用の範囲）。 */
export const uniqueIp = (): string => {
  ipCounter += 1;
  const n = (process.pid * 997 + ipCounter) % 65_000;
  return `198.${18 + Math.floor(n / 32_768)}.${Math.floor(n / 256) % 128}.${(n % 254) + 1}`;
};

/** 指定した秒数だけ待つ。 */
export const sleep = (seconds: number): Promise<void> => new Promise((r) => setTimeout(r, seconds * 1000));

export interface WorkspaceFixture {
  id: string;
  admin: Member;
  adminSession: Session;
  editor: Member;
  editorSession: Session;
  /** 公開済みの記事。タイトルと本文に `secretMarker` を含む。 */
  entryId: string;
  secretMarker: string;
  readKey: IssuedSecret;
  registerKey: IssuedSecret;
  listQuery: RegisteredQuery;
  byIdQuery: RegisteredQuery;
  /** 管理APIの操作の一覧を、このワークスペースに向けて呼ぶための対象。 */
  target: OperationTarget;
}

export interface World {
  cms: CmsClient;
  serverAdmin: Credentials;
  serverAdminSession: Session;
  a: WorkspaceFixture;
  b: WorkspaceFixture;
}

async function makeWorkspace(cms: CmsClient, serverAdmin: Credentials, serverAdminSession: Session, label: string): Promise<WorkspaceFixture> {
  await cms.reauthenticate(serverAdminSession, serverAdmin);
  const { id } = await cms.createWorkspace(serverAdminSession, `inv-${label}-${randomUUID().slice(0, 8)}`);

  const admin = await cms.createMember(serverAdminSession, id, "admin");
  const adminSession = await cms.login(admin);
  const editor = await cms.createMember(adminSession, id, "editor");
  const editorSession = await cms.login(editor);

  const secretMarker = marker(label);
  const entry = await cms.createEntry(editorSession, id, {
    title: `記事 ${secretMarker}`,
    body: simpleBody(`本文 ${secretMarker}`),
  });
  await cms.publishEntry(editorSession, id, entry.id);

  await cms.reauthenticate(adminSession, admin);
  const readKey = await cms.createApiKey(adminSession, id, ["read"]);
  const registerKey = await cms.createApiKey(adminSession, id, ["register"]);
  const listQuery = await cms.registerQuery(id, registerKey.secret, { kind: "list" });
  const byIdQuery = await cms.registerQuery(id, registerKey.secret, { kind: "byId", include: ["body"] });

  return {
    id,
    admin,
    adminSession,
    editor,
    editorSession,
    entryId: entry.id,
    secretMarker,
    readKey,
    registerKey,
    listQuery,
    byIdQuery,
    target: { workspaceId: id, entryId: entry.id, userId: editor.userId, apiKeyId: readKey.id },
  };
}

/**
 * 操作の一覧を呼ぶための使い捨ての対象（記事・利用者・APIキー）を、ワークスペースの中に新しく作る。
 * 状態を変える操作を試しても、舞台の記事やキーが壊れないようにするため。
 */
export async function disposableTarget(world: World, ws: WorkspaceFixture): Promise<OperationTarget> {
  const { cms } = world;
  const member = await cms.createMember(ws.adminSession, ws.id, "author");
  const entry = await cms.createEntry(ws.editorSession, ws.id, { title: marker("disposable"), body: simpleBody("使い捨て") });
  await cms.reauthenticate(ws.adminSession, ws.admin);
  const key = await cms.createApiKey(ws.adminSession, ws.id, ["read"]);
  return { workspaceId: ws.id, entryId: entry.id, userId: member.userId, apiKeyId: key.id };
}

export async function createWorld(): Promise<World> {
  const cms = driver.connect(env.cmsUrl);
  const serverAdmin = env.serverAdmin;
  const serverAdminSession = await cms.login(serverAdmin);
  const a = await makeWorkspace(cms, serverAdmin, serverAdminSession, "A");
  const b = await makeWorkspace(cms, serverAdmin, serverAdminSession, "B");
  return { cms, serverAdmin, serverAdminSession, a, b };
}

/** 最初に呼ばれたときに舞台を作り、以降は同じ舞台を返す（ファイルごとに1つ）。 */
export function lazyWorld(): () => Promise<World> {
  let promise: Promise<World> | null = null;
  return () => (promise ??= createWorld());
}

// ---------------------------------------------------------------------------
// サイト内検索の舞台（M3）
// ---------------------------------------------------------------------------

/** 本文（body）を検索の対象にしたテンプレートのワークスペース。記事は入っていない。 */
export interface SearchWorkspace {
  id: string;
  admin: Member;
  adminSession: Session;
  editor: Member;
  editorSession: Session;
  readKey: IssuedSecret;
  /** クエリを登録するためのキー。 */
  registerKey: IssuedSecret;
  /** 登録済みのサイト内検索のクエリ。 */
  searchQuery: RegisteredQuery;
}

export interface SearchWorld {
  cms: CmsClient;
  a: SearchWorkspace;
  b: SearchWorkspace;
}

async function makeSearchWorkspace(cms: CmsClient, serverAdmin: Credentials, serverAdminSession: Session, label: string): Promise<SearchWorkspace> {
  await cms.reauthenticate(serverAdminSession, serverAdmin);
  const { id } = await cms.createWorkspace(serverAdminSession, `inv-${label}-${randomUUID().slice(0, 8)}`, { template: SEARCHABLE_TEMPLATE.name });
  const admin = await cms.createMember(serverAdminSession, id, "admin");
  const adminSession = await cms.login(admin);
  const editor = await cms.createMember(adminSession, id, "editor");
  const editorSession = await cms.login(editor);
  await cms.reauthenticate(adminSession, admin);
  const readKey = await cms.createApiKey(adminSession, id, ["read"]);
  const registerKey = await cms.createApiKey(adminSession, id, ["register"]);
  const searchQuery = await cms.registerQuery(id, registerKey.secret, { kind: "search" });
  return { id, admin, adminSession, editor, editorSession, readKey, registerKey, searchQuery };
}

/**
 * 検索の舞台: テスト環境のCMSに SEARCHABLE_TEMPLATE をデプロイし、それを使う2つのワークスペース（A・B）を作る。
 * テスト環境のCMSは擬似LLMにつながっていて、埋め込みの予算の天井の既定値は十分に大きい（server-process.ts）。
 */
export async function createSearchWorld(): Promise<SearchWorld> {
  const cms = driver.connect(env.cmsUrl);
  const serverAdmin = env.serverAdmin;
  const serverAdminSession = await cms.login(serverAdmin);
  await cms.reauthenticate(serverAdminSession, serverAdmin);
  const token = await cms.deployToken(serverAdminSession, serverAdmin);
  const res = await cms.deployTemplateRaw(asBearer(token), SEARCHABLE_TEMPLATE);
  if (res.status < 200 || res.status >= 300) throw new Error(`SEARCHABLE_TEMPLATE をデプロイできない（${res.status}）: ${res.text}`);
  const a = await makeSearchWorkspace(cms, serverAdmin, serverAdminSession, "SA");
  const b = await makeSearchWorkspace(cms, serverAdmin, serverAdminSession, "SB");
  return { cms, a, b };
}

/** 最初に呼ばれたときに検索の舞台を作り、以降は同じ舞台を返す（ファイルごとに1つ）。 */
export function lazySearchWorld(): () => Promise<SearchWorld> {
  let promise: Promise<SearchWorld> | null = null;
  return () => (promise ??= createSearchWorld());
}

// ---------------------------------------------------------------------------
// 質問応答の舞台（M4）
// ---------------------------------------------------------------------------

/** 質問応答のワークスペース。記事は入っていない。 */
export interface AnswerWorkspace {
  id: string;
  editorSession: Session;
  readKey: IssuedSecret;
  /** 回答APIを呼ぶキー（answer のスコープだけ）。 */
  answerKey: IssuedSecret;
  searchQuery: RegisteredQuery;
  /** 登録済みの、まとめて返す回答のクエリ。 */
  answerQuery: RegisteredQuery;
  /** 登録済みの、ストリーミングの回答のクエリ。 */
  answerStreamQuery: RegisteredQuery;
}

export interface AnswerWorld {
  cms: CmsClient;
  /** このインスタンスが擬似LLMにつなぐキー（記録・障害・回答の組み立て方は、このキーに対して設定する）。 */
  llmKey: string;
  /**
   * ワークスペースを作る。埋め込みと回答の予算の天井は、answerCeiling を指定しなければ十分に大きくする。
   * answerCeiling は回答の予算の天井（トークン数。入力・出力、月・日のすべてに同じ値）。
   * visitorId を true にすると、回答のクエリを、訪問者の識別子の変数 visitorId も受け取る形で登録する。
   * cached を true にすると、回答のクエリで、回答キャッシュから返したか（cached）も選ぶ。
   */
  workspace(label: string, options?: { answerCeiling?: number; visitorId?: boolean; cached?: boolean }): Promise<AnswerWorkspace>;
  /** このインスタンスのサーバーの設定（driver.storage に渡す）。 */
  config: ServerConfig;
  stop(): Promise<void>;
}

/** 回答の予算の、十分に大きい天井。 */
const LARGE_BUDGET = 10_000_000;

/**
 * 質問応答の舞台: 専用のインスタンスを、専用のキーで擬似LLMにつないで起動する（擬似LLMの記録と回答の組み立て方は
 * キーごとなので、他のテストと混ざらない）。同じファイルのテストは順に流れるので、このキーを共有する。
 * 各テストは、使い終わったら擬似LLMの障害と回答の組み立て方を元に戻す。
 */
export async function createAnswerWorld(options: Pick<ServerConfig, "answerRateLimits"> = {}): Promise<AnswerWorld> {
  const llmKey = `inv-answer-${randomUUID().slice(0, 8)}`;
  const instance = await driver.createIsolatedInstance();
  // 舞台の準備のログインと招待の承諾は同じアドレスから続くので、テストの舞台のCMSと同じく回数制限を大きくする（global-setup.ts）
  const config: ServerConfig = {
    ...instance.config,
    loginAttemptsPerIpPerMinute: 100_000,
    llm: { ...mockLlm.connection(llmKey), chatModel: mockLlm.chatModel },
    ...options,
  };
  const started = await driver.startServer(config);
  if (started.outcome !== "ready") throw new Error(`擬似LLMにつないだサーバーが起動しない:\n${started.output}`);
  const cms = started.client!;
  const sa = await cms.login(instance.serverAdmin);
  await cms.reauthenticate(sa, instance.serverAdmin);
  const token = await cms.deployToken(sa, instance.serverAdmin);
  const res = await cms.deployTemplateRaw(asBearer(token), SEARCHABLE_TEMPLATE);
  if (res.status < 200 || res.status >= 300) throw new Error(`SEARCHABLE_TEMPLATE をデプロイできない（${res.status}）: ${res.text}`);
  return {
    cms,
    llmKey,
    config,
    async workspace(label, options = {}) {
      await cms.reauthenticate(sa, instance.serverAdmin);
      const { id } = await cms.createWorkspace(sa, `inv-${label}-${randomUUID().slice(0, 8)}`, { template: SEARCHABLE_TEMPLATE.name });
      const answer = options.answerCeiling ?? LARGE_BUDGET;
      await cms.setWorkspaceLimits(sa, id, {
        embeddingTokensPerMonth: LARGE_BUDGET,
        embeddingTokensPerDay: LARGE_BUDGET,
        answerInputTokensPerMonth: answer,
        answerInputTokensPerDay: answer,
        answerOutputTokensPerMonth: answer,
        answerOutputTokensPerDay: answer,
      });
      const admin = await cms.createMember(sa, id, "admin");
      const adminSession = await cms.login(admin);
      const editor = await cms.createMember(adminSession, id, "editor");
      const editorSession = await cms.login(editor);
      await cms.reauthenticate(adminSession, admin);
      const readKey = await cms.createApiKey(adminSession, id, ["read"]);
      const answerKey = await cms.createApiKey(adminSession, id, ["answer"]);
      const registerKey = await cms.createApiKey(adminSession, id, ["register"]);
      return {
        id,
        editorSession,
        readKey,
        answerKey,
        searchQuery: await cms.registerQuery(id, registerKey.secret, { kind: "search" }),
        answerQuery: await cms.registerQuery(id, registerKey.secret, { kind: "answer", visitorId: options.visitorId, cached: options.cached }),
        answerStreamQuery: await cms.registerQuery(id, registerKey.secret, { kind: "answerStream", visitorId: options.visitorId, cached: options.cached }),
      };
    },
    stop: () => started.stop(),
  };
}
