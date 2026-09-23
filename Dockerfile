FROM rust:1.88-bookworm AS web-build
WORKDIR /app/web
RUN apt-get update && apt-get install -y --no-install-recommends curl ca-certificates unzip \
    && rm -rf /var/lib/apt/lists/*
RUN curl -fsSL https://bun.sh/install | bash
ENV PATH="/root/.bun/bin:${PATH}"
COPY web/package.json web/package-lock.json* ./
RUN npm install
COPY web/ ./
RUN npm run build
RUN node -e "const fs=require('fs');const p='dist/runtime-config.js';try{let s=fs.readFileSync(p,'utf8');s=s.replace(/\"turnUsername\"\\s*:\\s*\"[^\"]*\"/g,'\"turnUsername\":\"\"').replace(/\"turnCredential\"\\s*:\\s*\"[^\"]*\"/g,'\"turnCredential\":\"\"').replace(/\"username\"\\s*:\\s*\"[^\"]*\"/g,'\"username\":\"\"').replace(/\"credential\"\\s*:\\s*\"[^\"]*\"/g,'\"credential\":\"\"');fs.writeFileSync(p,s);console.log('stripped TURN secrets from '+p);}catch(e){console.log('no runtime-config.js to strip: '+e.message);}"

FROM rust:1.88-bookworm AS server-build
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY rust ./rust
COPY files/config.example.toml ./files/config.example.toml
RUN mkdir -p files/uploads
RUN cargo build --release
COPY --from=web-build /app/web/dist ./web/dist

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
ENV PRODUCTION=1
COPY --from=server-build /app/target/release/qxprotocol /usr/local/bin/qxprotocol
RUN mkdir -p /app/files/uploads
COPY --from=server-build /app/web/dist ./web/dist
COPY --from=server-build /app/files/config.example.toml ./files/config.example.toml
EXPOSE 4560
CMD ["/usr/local/bin/qxprotocol"]
