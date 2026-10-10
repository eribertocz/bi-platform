FROM docker.io/library/node:24.21.0-slim AS frontend
WORKDIR /app/frontend
COPY frontend/ ./
RUN npm ci && npm run build

FROM docker.io/library/rust:1.99.0-slim-trixie AS backend
WORKDIR /app/backend
COPY backend/ ./
ARG BI_CARGO_FEATURES=""
RUN cargo build --locked --release -j 2 -p bi-server --features "$BI_CARGO_FEATURES"

FROM docker.io/library/debian:trixie-slim
WORKDIR /app
COPY --from=backend /app/backend/target/release/bi-server /usr/local/bin/bi-server
COPY --from=frontend /app/frontend/build /app/static
ENV BI_BIND_ADDR=0.0.0.0:8080 BI_STATIC_DIR=/app/static BI_LOG_FORMAT=json
USER 65532:65532
EXPOSE 8080
ENTRYPOINT ["bi-server"]
