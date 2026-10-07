# Dev tracker image (B6). Build from trader-scout/:
#   docker build -f deploy/Dockerfile -t trader-scout/dev-tracker .
# Secrets (database URL, RPC URLs, API keys, Telegram token) are passed as
# environment variables at run time and never baked into the image.
FROM rust:1.98.1-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release --locked -p dev-tracker \
 && cp target/release/dev-tracker /dev-tracker

FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --system --uid 10001 --home /app scout \
 && mkdir -p /app/config /app/exports \
 && chown -R scout /app
COPY --from=build /dev-tracker /usr/local/bin/dev-tracker
COPY config/dev-tracker.example.toml /app/config/dev-tracker.toml
WORKDIR /app
USER scout
ENTRYPOINT ["dev-tracker"]
CMD ["run", "--config", "/app/config/dev-tracker.toml"]
