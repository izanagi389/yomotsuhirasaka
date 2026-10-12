// DBの権限の約束（INV-DB-*・INV-CFG-03）のテストだけが使う、DBへの直接の接続。

import pg from "pg";
import { expect } from "vitest";

const { Client, escapeIdentifier } = pg;

export const ident = escapeIdentifier;

/** 「スキーマ名.テーブル名」をクォートしたもの。 */
export const qualified = (schema: string, table: string): string => `${ident(schema)}.${ident(table)}`;

export async function withClient<T>(url: string, fn: (client: pg.Client) => Promise<T>): Promise<T> {
  const client = new Client({ connectionString: url });
  await client.connect();
  try {
    return await fn(client);
  } finally {
    await client.end();
  }
}

export interface TableInfo {
  schema: string;
  table: string;
  /** 先頭の列の名前（UPDATEの確認に使う）。 */
  firstColumn: string;
}

/** 指定したスキーマのテーブルの一覧（スーパーユーザーの接続で調べる）。 */
export async function listTables(superuserUrl: string, schemas: string[]): Promise<TableInfo[]> {
  return withClient(superuserUrl, async (c) => {
    const res = await c.query<{ schema: string; table: string; first_column: string }>(
      `SELECT c.table_schema AS schema, c.table_name AS table, c.column_name AS first_column
         FROM information_schema.columns c
         JOIN information_schema.tables t
           ON t.table_schema = c.table_schema AND t.table_name = c.table_name AND t.table_type = 'BASE TABLE'
        WHERE c.table_schema = ANY($1) AND c.ordinal_position = 1
        ORDER BY 1, 2`,
      [schemas],
    );
    return res.rows.map((r) => ({ schema: r.schema, table: r.table, firstColumn: r.first_column }));
  });
}

export const PERMISSION_DENIED = "42501";

/**
 * 1つのトランザクションの中で（必要ならロールを切り替えて）SQLを実行し、権限の不足で失敗することを確かめる。
 * 想定に反して成功しても、最後にロールバックするのでDBは変わらない。
 */
export async function expectPermissionDenied(client: pg.Client, role: string | null, sql: string, what: string): Promise<void> {
  await client.query("BEGIN");
  try {
    if (role) await client.query(`SET LOCAL ROLE ${ident(role)}`);
    let code: string | undefined;
    try {
      await client.query(sql);
    } catch (e) {
      code = (e as { code?: string }).code;
    }
    expect(code, `${what} は権限の不足（SQLSTATE ${PERMISSION_DENIED}）で失敗するべき。実行したSQL: ${sql}`).toBe(PERMISSION_DENIED);
  } finally {
    await client.query("ROLLBACK");
  }
}

/** 1つのトランザクションの中で（必要ならロールを切り替えて）SQLを実行し、成功することを確かめる（対照用）。 */
export async function expectAllowed(client: pg.Client, role: string | null, sql: string, what: string): Promise<void> {
  await client.query("BEGIN");
  try {
    if (role) await client.query(`SET LOCAL ROLE ${ident(role)}`);
    await expect(client.query(sql), `${what} は成功するべき。実行したSQL: ${sql}`).resolves.toBeDefined();
  } finally {
    await client.query("ROLLBACK");
  }
}
