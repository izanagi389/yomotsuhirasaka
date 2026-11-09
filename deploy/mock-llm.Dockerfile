# 擬似LLMのイメージ（開発用。deploy/docker-compose.dev.yml。12-dev-environment.md）。本番では使わない。
# ビルドの文脈はリポジトリのルート。

FROM node:26-trixie-slim@sha256:5adad64f9a1fca2d51972caa65983634c8a774eb300d72b20f7f2a0f60368072 AS build
WORKDIR /src
COPY package.json package-lock.json ./
COPY tools/mock-llm/package.json tools/mock-llm/
RUN npm ci --ignore-scripts -w @yomotsuhirasaka/mock-llm
COPY tools/mock-llm ./tools/mock-llm
RUN npm run build -w @yomotsuhirasaka/mock-llm

# 実行時の依存はない（Node.js の標準の機能だけで動く）
FROM node:26-trixie-slim@sha256:5adad64f9a1fca2d51972caa65983634c8a774eb300d72b20f7f2a0f60368072
WORKDIR /app
COPY --from=build /src/tools/mock-llm/package.json ./
COPY --from=build /src/tools/mock-llm/dist ./dist
USER node
ENV PORT=8080 HOST=0.0.0.0
EXPOSE 8080
CMD ["node", "dist/main.js"]
