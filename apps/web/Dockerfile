FROM node:24-bookworm-slim AS deps

WORKDIR /app
RUN corepack enable
COPY package.json pnpm-lock.yaml pnpm-workspace.yaml ./
COPY apps/web/package.json ./apps/web/package.json
COPY apps/web/svelte.config.js ./apps/web/svelte.config.js
RUN pnpm install --frozen-lockfile --filter @deepref/web...

FROM node:24-bookworm-slim AS builder

WORKDIR /app
RUN corepack enable
COPY --from=deps /app/node_modules ./node_modules
COPY --from=deps /app/apps/web/node_modules ./apps/web/node_modules
COPY . .
RUN pnpm --filter @deepref/web build

FROM caddy:2-alpine AS runtime

COPY apps/web/Caddyfile /etc/caddy/Caddyfile
COPY apps/web/docker-entrypoint.sh /usr/local/bin/docker-entrypoint.sh
COPY --from=builder /app/apps/web/build /usr/share/caddy

RUN chmod +x /usr/local/bin/docker-entrypoint.sh

EXPOSE 80
ENTRYPOINT ["/usr/local/bin/docker-entrypoint.sh"]
CMD ["caddy", "run", "--config", "/etc/caddy/Caddyfile", "--adapter", "caddyfile"]
