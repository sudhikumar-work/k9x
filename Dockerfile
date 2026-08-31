# Multi-stage minimal build for k9x
FROM rust:1.85-alpine AS builder

RUN apk add --no-cache musl-dev gcc make binutils

WORKDIR /usr/src/k9x
COPY Cargo.toml Cargo.lock ./
COPY src ./src

# Build statically linked release binary in native musl environment
RUN cargo build --release && \
    cp target/release/k9x /usr/local/bin/k9x && \
    strip /usr/local/bin/k9x

# Final minimal runtime image
FROM alpine:3.21

RUN apk add --no-cache ca-certificates tzdata

COPY --from=builder /usr/local/bin/k9x /usr/local/bin/k9x

ENTRYPOINT ["/usr/local/bin/k9x"]
