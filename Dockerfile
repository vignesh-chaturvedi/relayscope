# One image with the relayscope binary and the built web app and overlay.
#   docker compose up                       (see docker-compose.yml)
#   docker build -t relayscope . && docker run --rm relayscope --help

# The web app and the stream overlay.
FROM node:24-slim AS web
WORKDIR /src/web
COPY web/package.json web/package-lock.json ./
RUN --mount=type=cache,target=/root/.npm npm ci
COPY web ./
RUN npm run build

# The binary. The Solami SDK behind gapless builds protoc from source, which needs cmake.
FROM rust:1-bookworm AS build
RUN apt-get update && apt-get install -y --no-install-recommends cmake && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p relayscope && \
    cp target/release/relayscope /usr/local/bin/relayscope && \
    mkdir -p /empty

# Runtime: glibc, CA certificates and the binary. No shell, and not root.
FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=build /usr/local/bin/relayscope /usr/local/bin/relayscope
COPY --from=web /src/web/dist /app/web/dist
# The database directory, owned by the nonroot user so a new volume is writable.
COPY --from=build --chown=nonroot:nonroot /empty /data
ENV RELAYSCOPE_DB=/data/relayscope.db \
    RELAYSCOPE_WEB=/app/web/dist \
    RELAYSCOPE_LISTEN=0.0.0.0:8790
WORKDIR /app
VOLUME /data
EXPOSE 8790
ENTRYPOINT ["relayscope"]
CMD ["serve"]
