# CMSのサーバーのイメージ（公式の Docker Compose で使う。deploy/README.md）。
# ビルドの文脈はリポジトリのルート（deploy/docker-compose.yml の build.context）。

# 管理画面（Nuxt の SPA）。静的なファイルにしてサーバーのイメージに入れる。本番で Node.js は動かさない
FROM node:22-trixie-slim AS admin
WORKDIR /src
# ルートの npm workspaces（管理画面と、共有パッケージ）
COPY package.json package-lock.json ./
COPY admin/package.json admin/
COPY packages/editor-schema/package.json packages/editor-schema/
RUN npm ci --ignore-scripts
COPY packages ./packages
COPY admin ./admin
WORKDIR /src/admin
RUN npx nuxt prepare && npm run generate

FROM rust:1.97-slim-trixie AS build
# パスキーの照合（webauthn-rs）が OpenSSL を使う
RUN apt-get update -q && apt-get install -y --no-install-recommends pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY server ./server
RUN cargo build --release --locked -p cms-server

FROM debian:trixie-slim
RUN apt-get update -q && apt-get install -y --no-install-recommends libssl3t64 ca-certificates && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --home-dir /data --shell /usr/sbin/nologin cms
COPY --from=build /src/target/release/cms-server /usr/local/bin/cms-server
# 設定の [admin] dist_dir で指す（deploy/init.sh が cms-server init --admin-dist-dir で書く）
COPY --from=admin /src/admin/.output/public /usr/share/cms-admin
# 設定ファイル・暗号化の鍵・メディアは /data に置く（ホストのディレクトリをつなぐ）
VOLUME ["/data"]
USER 10001
EXPOSE 8080
ENTRYPOINT ["cms-server"]
CMD ["serve", "--config", "/data/config.toml"]
