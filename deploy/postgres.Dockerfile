# 拡張を入れた PostgreSQL 16 のイメージ（01-architecture.md「DB」、Q23）。
# - pgvector: PostgreSQL の公式の apt のリポジトリ（PGDG）のパッケージ
# - pg_bigm: PGDG にないため、公開されているソースを決まったコミットでビルドする
# 拡張を使う機能（検索・埋め込み）はM3から。初期化のスクリプト（deploy/init.sh）が、拡張を yomi のDBに作る。

# 外部のイメージは digest で固定する（13-security.md）。postgres:16-trixie のタグの、マルチアーキテクチャの一覧の digest
FROM postgres:18-trixie@sha256:e3d24f6906d0760bf4da0dae872415391ededefad23fd471c47dc53b0ddf3c42

# pg_bigm v1.2-20250903（https://github.com/pgbigm/pg_bigm、PostgreSQL License）
ARG PG_BIGM_COMMIT=735dceba0ecdd8ac1aaaaa207226a7102b6bbd71

RUN set -eux; \
    apt-get update -q; \
    apt-get install -y --no-install-recommends postgresql-16-pgvector; \
    apt-get install -y --no-install-recommends git ca-certificates build-essential postgresql-server-dev-16 libicu-dev; \
    git clone --filter=blob:none https://github.com/pgbigm/pg_bigm.git /tmp/pg_bigm; \
    git -C /tmp/pg_bigm checkout --detach "$PG_BIGM_COMMIT"; \
    make -C /tmp/pg_bigm USE_PGXS=1; \
    make -C /tmp/pg_bigm USE_PGXS=1 install; \
    rm -rf /tmp/pg_bigm; \
    apt-get purge -y --auto-remove git build-essential postgresql-server-dev-16 libicu-dev; \
    rm -rf /var/lib/apt/lists/*
