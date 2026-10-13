-- 配信APIの登録済みクエリ（Persisted Queries。02-schema-and-delivery-api.md、13-security.md「GraphQL」、Q94, Q151）。
CREATE TABLE queries (
    -- クエリの文のSHA-256（16進数64桁）。配信のリクエストはこの値で指す
    id             text PRIMARY KEY CHECK (id ~ '^[0-9a-f]{64}$'),
    query          text NOT NULL,
    -- 登録したAPIキー
    registered_by  uuid,
    registered_at  timestamptz NOT NULL DEFAULT now(),
    -- 最後に使った（実行した・登録し直した）日時。長く使われていないものは、デプロイの検証の対象から外す（Q151）
    last_used_at   timestamptz NOT NULL DEFAULT now()
);
