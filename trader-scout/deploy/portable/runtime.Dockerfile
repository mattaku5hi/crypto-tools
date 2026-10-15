FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/* \
 && mkdir -p /app/config /app/exports
COPY bin/dev-tracker /usr/local/bin/dev-tracker
WORKDIR /app
ENTRYPOINT ["dev-tracker"]
CMD ["run", "--config", "/app/config/dev-tracker.toml"]
