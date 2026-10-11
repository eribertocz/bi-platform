FROM docker.io/library/node:24.21.0-slim AS frontend
WORKDIR /app/frontend
COPY frontend/ ./
RUN npm ci && npm run build

FROM docker.io/library/rust:1.99.0-slim-trixie AS backend
# The official prebuilt DuckDB engine of the version duckdb-rs pins, verified
# by checksum, instead of compiling its C++ sources on every image build.
# Changing DUCKDB_VERSION requires new checksums and the matching duckdb crate.
ARG DUCKDB_VERSION=1.5.6
ARG TARGETARCH
RUN set -eu; \
    apt-get update; \
    apt-get install -y --no-install-recommends curl unzip; \
    rm -rf /var/lib/apt/lists/*; \
    case "${TARGETARCH:-amd64}" in \
      amd64) sum=b845005f5132a7d8180057c35e14a7626632258782f871a90861b19c1c03841b ;; \
      arm64) sum=b72ed9f05003f5e9d2015f7ceada6416b377d9dd33169cdde3c9e33897856eee ;; \
      *) echo "unsupported architecture: ${TARGETARCH}" >&2; exit 1 ;; \
    esac; \
    curl -fsSL -o /tmp/libduckdb.zip \
      "https://github.com/duckdb/duckdb/releases/download/v${DUCKDB_VERSION}/libduckdb-linux-${TARGETARCH:-amd64}.zip"; \
    echo "${sum}  /tmp/libduckdb.zip" | sha256sum -c -; \
    mkdir -p /opt/duckdb; \
    unzip -q /tmp/libduckdb.zip -d /opt/duckdb; \
    rm /tmp/libduckdb.zip
ENV DUCKDB_LIB_DIR=/opt/duckdb DUCKDB_INCLUDE_DIR=/opt/duckdb
WORKDIR /app/backend
COPY backend/ ./
ARG BI_CARGO_FEATURES=""
# Cache mounts keep the crate registry and compiled dependencies between
# builds, so a code change recompiles only our crates. `locked`: builds
# from several checkouts on one machine take turns instead of racing.
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/app/backend/target,sharing=locked \
    cargo build --locked --release -j 2 -p bi-server --no-default-features --features "$BI_CARGO_FEATURES" \
    && cp target/release/bi-server /usr/local/bin/bi-server

FROM docker.io/library/debian:trixie-slim
# CA certificates: DuckDB downloads its ducklake/postgres extensions over HTTPS.
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && mkdir -p /var/lib/bi/lake /var/lib/bi/duckdb-extensions \
    && chown -R 65532:65532 /var/lib/bi
COPY --from=backend /opt/duckdb/libduckdb.so /usr/local/lib/libduckdb.so
RUN ldconfig
WORKDIR /app
COPY --from=backend /usr/local/bin/bi-server /usr/local/bin/bi-server
COPY --from=frontend /app/frontend/build /app/static
ENV BI_BIND_ADDR=0.0.0.0:8080 BI_STATIC_DIR=/app/static BI_LOG_FORMAT=json \
    BI_LAKE_DATA_PATH=/var/lib/bi/lake BI_DUCKDB_EXTENSION_DIR=/var/lib/bi/duckdb-extensions
USER 65532:65532
EXPOSE 8080
ENTRYPOINT ["bi-server"]
