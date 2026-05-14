FROM rust:1.88-bookworm AS web-build
WORKDIR /app/web
COPY web/package.json web/package-lock.json* ./
RUN npm install
COPY web/ ./
RUN npm run build

FROM rust:1.88-bookworm AS server-build
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY rust ./rust
COPY files ./files
RUN cargo build --release
COPY --from=web-build /app/web/dist ./web/dist

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
ENV PRODUCTION=1
COPY --from=server-build /app/target/release/qxprotocol /usr/local/bin/qxprotocol
COPY --from=server-build /app/files ./files
COPY --from=server-build /app/web/dist ./web/dist
EXPOSE 4560
CMD ["/usr/local/bin/qxprotocol"]
