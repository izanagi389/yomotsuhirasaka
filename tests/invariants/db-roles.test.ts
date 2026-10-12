// DBの権限の約束: INV-DB-01, 02, 03, 04, INV-CFG-03
//
// CMSが普段の処理に使うのと同じ接続（CMS_TEST_DB_APP_URL）でDBに直接つなぎ、
// CMSと同じようにワークスペースのロールに切り替えて、権限の不足で失敗することを確かめる。
// どのSQLもトランザクションの中で実行し、最後にロールバックする。

import { expect } from "vitest";
import { inv } from "./support/inv.js";
import { driver } from "./support/driver.js";
import { env } from "./support/env.js";
import { expectAllowed, expectPermissionDenied, ident, listTables, qualified, withClient } from "./support/db.js";
import { lazyWorld } from "./support/world.js";

const world = lazyWorld();
const { db } = driver;

inv("inv_db_01", "あるワークスペースの処理に使うDBの権限では、他のワークスペースのテーブルを読み書きできない", async () => {
  const { a, b } = await world();
  const roleA = await db.workspaceRole(a.id);
  const tablesA = await listTables(env.dbSuperuserUrl, [await db.workspaceSchema(a.id)]);
  const tablesB = await listTables(env.dbSuperuserUrl, [await db.workspaceSchema(b.id)]);
  expect(tablesA.length, "Aのスキーマにテーブルがない").toBeGreaterThan(0);
  expect(tablesB.length, "Bのスキーマにテーブルがない").toBeGreaterThan(0);

  await withClient(env.dbAppUrl, async (c) => {
    // 対照: Aのロールで、Aのテーブルは読める
    for (const t of tablesA) {
      await expectAllowed(c, roleA, `SELECT * FROM ${qualified(t.schema, t.table)} LIMIT 1`, `Aのロールで ${t.table} を読む`);
    }
    for (const t of tablesB) {
      const name = qualified(t.schema, t.table);
      const col = ident(t.firstColumn);
      await expectPermissionDenied(c, roleA, `SELECT * FROM ${name} LIMIT 1`, `Aのロールで、Bの ${t.table} を読む`);
      await expectPermissionDenied(c, roleA, `INSERT INTO ${name} DEFAULT VALUES`, `Aのロールで、Bの ${t.table} に追加する`);
      await expectPermissionDenied(c, roleA, `UPDATE ${name} SET ${col} = ${col}`, `Aのロールで、Bの ${t.table} を更新する`);
      await expectPermissionDenied(c, roleA, `DELETE FROM ${name}`, `Aのロールで、Bの ${t.table} を削除する`);
    }
  });
});

inv("inv_db_02", "普段の処理に使うDBの権限では、テーブルの作成・変更・削除ができない", async () => {
  const { a } = await world();
  const roleA = await db.workspaceRole(a.id);
  const schemaA = await db.workspaceSchema(a.id);
  const shared = await db.sharedSchemas();
  const tables = await listTables(env.dbSuperuserUrl, [schemaA, ...shared]);
  expect(tables.length).toBeGreaterThan(0);

  await withClient(env.dbAppUrl, async (c) => {
    // 普段の接続のロールのまま（null）と、ワークスペースのロールに切り替えた状態の両方
    for (const role of [null, roleA]) {
      const who = role ? "ワークスペースのロール" : "普段の接続のロール";
      for (const schema of [schemaA, ...shared, "public"]) {
        await expectPermissionDenied(c, role, `CREATE TABLE ${qualified(schema, "inv_db_02_tmp")} (x int)`, `${who}で ${schema} にテーブルを作成`);
      }
      for (const t of tables) {
        const name = qualified(t.schema, t.table);
        await expectPermissionDenied(c, role, `ALTER TABLE ${name} ADD COLUMN inv_db_02_tmp int`, `${who}で ${t.schema}.${t.table} を変更`);
        await expectPermissionDenied(c, role, `DROP TABLE ${name}`, `${who}で ${t.schema}.${t.table} を削除`);
      }
    }
  });
});

inv("inv_db_03", "普段の処理に使うDBの権限では、監査ログを更新・削除できない", async () => {
  const { a } = await world();
  const roleA = await db.workspaceRole(a.id);
  const auditTables = await db.auditLogTables();
  expect(auditTables.length, "監査ログのテーブルがない").toBeGreaterThan(0);
  const infos = await listTables(env.dbSuperuserUrl, [...new Set(auditTables.map((t) => t.split(".")[0]!))]);

  await withClient(env.dbAppUrl, async (c) => {
    for (const full of auditTables) {
      const [schema, table] = full.split(".") as [string, string];
      const info = infos.find((t) => t.schema === schema && t.table === table);
      expect(info, `監査ログのテーブル ${full} が見つからない`).toBeDefined();
      const name = qualified(schema, table);
      const col = ident(info!.firstColumn);
      for (const role of [null, roleA]) {
        const who = role ? "ワークスペースのロール" : "普段の接続のロール";
        await expectPermissionDenied(c, role, `UPDATE ${name} SET ${col} = ${col}`, `${who}で ${full} を更新`);
        await expectPermissionDenied(c, role, `DELETE FROM ${name}`, `${who}で ${full} を削除`);
        await expectPermissionDenied(c, role, `TRUNCATE ${name}`, `${who}で ${full} を空にする`);
      }
    }
  });
});

inv("inv_db_04", "ワークスペースの処理に使うDBの権限では、共通のテーブル（利用者、セッション、キー、監査ログなど）を読み書きできない", async () => {
  const { a } = await world();
  const roleA = await db.workspaceRole(a.id);
  const tables = await listTables(env.dbSuperuserUrl, await db.sharedSchemas());
  // 利用者、セッション、キー、監査ログの少なくとも4つはある
  expect(tables.length, "共通のテーブルが少なすぎる").toBeGreaterThanOrEqual(4);

  await withClient(env.dbAppUrl, async (c) => {
    for (const t of tables) {
      const name = qualified(t.schema, t.table);
      const col = ident(t.firstColumn);
      await expectPermissionDenied(c, roleA, `SELECT * FROM ${name} LIMIT 1`, `ワークスペースのロールで ${t.table} を読む`);
      await expectPermissionDenied(c, roleA, `INSERT INTO ${name} DEFAULT VALUES`, `ワークスペースのロールで ${t.table} に追加する`);
      await expectPermissionDenied(c, roleA, `UPDATE ${name} SET ${col} = ${col}`, `ワークスペースのロールで ${t.table} を更新する`);
      await expectPermissionDenied(c, roleA, `DELETE FROM ${name}`, `ワークスペースのロールで ${t.table} を削除する`);
    }
  });
});

inv("inv_cfg_03", "普段の処理用のDBのロールでは、ロールの作成とDDLができない（移行・ロールの作成は専用の接続だけが行う）", async () => {
  const { a } = await world();
  const roleA = await db.workspaceRole(a.id);

  await withClient(env.dbAppUrl, async (c) => {
    // 普段の接続のロールの属性
    const attrs = await c.query<{ rolsuper: boolean; rolcreaterole: boolean; rolcreatedb: boolean }>(
      "SELECT rolsuper, rolcreaterole, rolcreatedb FROM pg_roles WHERE rolname = current_user",
    );
    expect(attrs.rows[0], "普段の接続のロールの属性").toEqual({ rolsuper: false, rolcreaterole: false, rolcreatedb: false });

    for (const role of [null, roleA]) {
      const who = role ? "ワークスペースのロール" : "普段の接続のロール";
      await expectPermissionDenied(c, role, "CREATE ROLE inv_cfg_03_tmp", `${who}でロールを作成`);
      await expectPermissionDenied(c, role, "CREATE SCHEMA inv_cfg_03_tmp", `${who}でスキーマを作成`);
      await expectPermissionDenied(c, role, `CREATE TABLE ${qualified("public", "inv_cfg_03_tmp")} (x int)`, `${who}でテーブルを作成`);
    }
  });
});
