# 擬似LLMのイメージ（開発用。deploy/docker-compose.dev.yml。12-dev-environment.md）。本番では使わない。
# ビルドの文脈はリポジトリのルート。

FROM node:22-trixie-slim AS build
WORKDIR /src
COPY package.json package-lock.json ./
COPY tools/mock-llm/package.json tools/mock-llm/
RUN npm ci --ignore-scripts -w @new-cms/mock-llm
COPY tools/mock-llm ./tools/mock-llm
RUN npm run build -w @new-cms/mock-llm

# 実行時の依存はない（Node.js の標準の機能だけで動く）
FROM node:22-trixie-slim
WORKDIR /app
COPY --from=build /src/tools/mock-llm/package.json ./
COPY --from=build /src/tools/mock-llm/dist ./dist
USER node
ENV PORT=8080 HOST=0.0.0.0
EXPOSE 8080
CMD ["node", "dist/main.js"]
