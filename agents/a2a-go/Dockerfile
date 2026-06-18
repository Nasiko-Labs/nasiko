FROM golang:1.25-alpine AS builder

RUN apk add --no-cache ca-certificates

WORKDIR /src
COPY go.mod go.sum ./
RUN go mod download
COPY . .
RUN CGO_ENABLED=0 go build -ldflags="-s -w" -o /agent ./cmd/...

FROM scratch
COPY --from=builder /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/
COPY --from=builder /agent /agent
EXPOSE 8000
ENTRYPOINT ["/agent", "-port", "8000"]
