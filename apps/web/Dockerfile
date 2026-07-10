## Build stage
FROM rust:1-bookworm AS builder

RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY . .
# sqlx's query!/query_scalar! macros normally verify SQL against a live DB at
# compile time; SQLX_OFFLINE builds against the committed .sqlx/ cache
# instead (regenerate it with `cargo sqlx prepare` after changing a query).
ENV SQLX_OFFLINE=true
RUN cargo build --release --bin shovelsup-web

## Runtime stage
FROM debian:bookworm-slim

## REQ-002: pdftotext/pdftoppm (poppler-utils) and tesseract (with English +
## French language data, since REQ-007 requires French extraction) are
## shelled out to by src/pipeline/parser/pdf.rs and ocr.rs at runtime.
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    libssl3 \
    poppler-utils \
    tesseract-ocr \
    tesseract-ocr-fra \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --create-home --shell /usr/sbin/nologin appuser

WORKDIR /app
COPY --from=builder /app/target/release/shovelsup-web ./shovelsup-web
COPY templates ./templates
COPY static ./static

USER appuser

EXPOSE 3000
ENTRYPOINT ["./shovelsup-web"]
