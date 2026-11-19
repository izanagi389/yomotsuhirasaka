# 擬似LLMのイメージ（開発用。deploy/docker-compose.dev.yml。12-dev-environment.md）。本番では使わない。
# ビルドの文脈はリポジトリのルート。

FROM node:26-trixie-slim@sha256:e6bedef217677b276c591795e169840edd9096dbf13f29df8ef8d1856b8820bc AS build
WORKDIR /src
COPY package.json package-lock.json ./
COPY tools/mock-llm/package.json tools/mock-llm/
RUN npm ci --ignore-scripts -w @yomotsuhirasaka/mock-llm
COPY tools/mock-llm ./tools/mock-llm
RUN npm run build -w @yomotsuhirasaka/mock-llm

# 実行時の依存はない（Node.js の標準の機能だけで動く）
FROM node:26-trixie-slim@sha256:e6bedef217677b276c591795e169840edd9096dbf13f29df8ef8d1856b8820bc
WORKDIR /app
COPY --from=build /src/tools/mock-llm/package.json ./
COPY --from=build /src/tools/mock-llm/dist ./dist
USER node
ENV PORT=8080 HOST=0.0.0.0
EXPOSE 8080
CMD ["node", "dist/main.js"]
