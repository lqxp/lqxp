FROM rust:1.88-bookworm AS server-build
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY rust ./rust
COPY files/config.example.toml ./files/config.example.toml
RUN mkdir -p files/uploads
RUN cargo build --release

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl git unzip \
    && rm -rf /var/lib/apt/lists/*
ENV BUN_INSTALL=/opt/bun
RUN curl -fsSL https://bun.sh/install | bash
ENV PATH="/opt/bun/bin:${PATH}"
WORKDIR /app
ENV PRODUCTION=1

ENV QXP_ROOT=/app
COPY --from=server-build /app/target/release/qxprotocol /usr/local/bin/qxprotocol
RUN mkdir -p /app/files/uploads
COPY --from=server-build /app/files/config.example.toml ./files/config.example.toml
EXPOSE 4560
CMD ["/usr/local/bin/qxprotocol"]
