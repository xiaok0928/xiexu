FROM node:22-alpine AS web-build
WORKDIR /src/apps/web
COPY apps/web/package.json apps/web/package-lock.json* ./
RUN npm install
COPY apps/web ./
RUN npm run build

FROM rust:1.85-slim-bookworm AS rust-build
WORKDIR /src
COPY Cargo.toml ./
COPY apps ./apps
COPY crates ./crates
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/src/target,sharing=locked \
    cargo build --release --bin xiexu-server --bin xiexu-runner --bin xiexu-migrate \
    && mkdir -p /out \
    && cp /src/target/release/xiexu-server /src/target/release/xiexu-runner /src/target/release/xiexu-migrate /out/

FROM node:22-bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends git \
    && rm -rf /var/lib/apt/lists/* \
    && npm install -g @openai/codex@0.147.0 \
    && npm cache clean --force
COPY --from=rust-build /out/xiexu-server /usr/local/bin/xiexu-server
COPY --from=rust-build /out/xiexu-runner /usr/local/bin/xiexu-runner
COPY --from=rust-build /out/xiexu-migrate /usr/local/bin/xiexu-migrate
COPY --from=web-build /src/apps/web/dist /app/web
COPY apps/app-entrypoint.sh /usr/local/bin/xiexu-app-entrypoint
RUN chmod +x /usr/local/bin/xiexu-app-entrypoint
ENTRYPOINT ["/usr/local/bin/xiexu-app-entrypoint"]
