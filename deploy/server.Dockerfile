# CMSのサーバーのイメージ（公式の Docker Compose で使う。deploy/README.md）。
# ビルドの文脈はリポジトリのルート（deploy/docker-compose.yml の build.context）。

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
# 設定ファイル・暗号化の鍵・メディアは /data に置く（ホストのディレクトリをつなぐ）
VOLUME ["/data"]
USER 10001
EXPOSE 8080
ENTRYPOINT ["cms-server"]
CMD ["serve", "--config", "/data/config.toml"]
