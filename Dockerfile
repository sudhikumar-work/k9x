# Minimal multi-arch container image for k9x
FROM alpine:3.21

ARG TARGETARCH

RUN apk add --no-cache ca-certificates tzdata

COPY dist/${TARGETARCH}/k9x /usr/local/bin/k9x
RUN chmod +x /usr/local/bin/k9x

ENTRYPOINT ["/usr/local/bin/k9x"]

