FROM golang:1.25-alpine AS builder

RUN apk add --no-cache ca-certificates

# Compile-time OTel auto-instrumentation (loongsuite-go-agent): `otel go build`
# injects tracing into net/http and the OTLP exporter reads the standard
# OTEL_* env vars at runtime — no code changes in the agent.
ARG TARGETARCH=amd64
ADD https://github.com/alibaba/loongsuite-go-agent/releases/download/v1.12.0/otel-linux-${TARGETARCH} /usr/local/bin/otel
RUN chmod +x /usr/local/bin/otel

WORKDIR /src
COPY go.mod go.sum ./
RUN go mod download
COPY . .
RUN CGO_ENABLED=0 otel go build -ldflags="-s -w" -o /agent .

FROM scratch
COPY --from=builder /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/
COPY --from=builder /agent /agent
EXPOSE 8000
ENTRYPOINT ["/agent", "-port", "8000"]
