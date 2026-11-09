# CMSのサーバーのイメージ（公式の Docker Compose で使う。deploy/README.md）。
# ビルドの文脈はリポジトリのルート（deploy/docker-compose.yml の build.context）。

# 管理画面（Nuxt の SPA）。静的なファイルにしてサーバーのイメージに入れる。本番で Node.js は動かさない
FROM node:26-trixie-slim@sha256:5adad64f9a1fca2d51972caa65983634c8a774eb300d72b20f7f2a0f60368072 AS admin
WORKDIR /src
# ルートの npm workspaces（管理画面と、共有パッケージ）
COPY package.json package-lock.json ./
COPY admin/package.json admin/
COPY cli/package.json cli/
COPY packages/editor-schema/package.json packages/editor-schema/
COPY packages/nuxt/package.json packages/nuxt/
COPY packages/preview/package.json packages/preview/
RUN npm ci --ignore-scripts
COPY packages ./packages
# 管理画面が使う共有パッケージ（本文の部品の定義と、ライブプレビューのメッセージの決まり）
RUN npm run build -w @yomotsuhirasaka/editor-schema && npm run build -w @yomotsuhirasaka/preview
COPY admin ./admin
WORKDIR /src/admin
RUN npx nuxt prepare && npm run generate

FROM rust:1.98-slim-trixie@sha256:4cd829461bd5c4d511c32e269da9cb8929223b666519d8004e35fc8d1d771ab7 AS build
# パスキーの照合（webauthn-rs）が OpenSSL を使う
RUN apt-get update -q && apt-get install -y --no-install-recommends pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY server ./server
# 質問応答のプロンプトと回答の形（擬似LLMと同じファイルを読む。server/src/answer/format.rs）
COPY tools/mock-llm/src/format.json ./tools/mock-llm/src/format.json
RUN cargo build --release --locked -p yomi-server

# 配布するバイナリの中身（.github/workflows/release.yml が `--target dist --output type=local` で取り出す）
FROM scratch AS dist
COPY --from=build /src/target/release/yomi-server /yomi-server
COPY --from=admin /src/admin/.output/public /admin

FROM debian:trixie-slim@sha256:a72b4433de1cf7d6bae0c4cee74d0547f58601a643b3375f070c51600d63b6b2
RUN apt-get update -q && apt-get install -y --no-install-recommends libssl3t64 ca-certificates && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --home-dir /data --shell /usr/sbin/nologin yomi
COPY --from=build /src/target/release/yomi-server /usr/local/bin/yomi-server
# 設定の [admin] dist_dir で指す（deploy/init.sh が yomi-server init --admin-dist-dir で書く）
COPY --from=admin /src/admin/.output/public /usr/share/yomi-admin
# 設定ファイル・暗号化の鍵・メディアは /data に置く（ホストのディレクトリをつなぐ）
VOLUME ["/data"]
USER 10001
EXPOSE 8080
ENTRYPOINT ["yomi-server"]
CMD ["serve", "--config", "/data/config.toml"]
