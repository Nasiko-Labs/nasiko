# The binary is cross-compiled on the HOST (see `just stage`) and copied in,
# rather than compiled inside the image. Building Rust for linux/amd64 on an
# arm64 dev machine would run the whole compile under QEMU emulation — slow and
# a recurring source of flaky builds. `dist/linux/<arch>/` is populated by the
# justfile; TARGETARCH picks the right one for each platform in the manifest
# list, so one Dockerfile serves both.
FROM alpine:latest AS certs
RUN apk --no-cache add ca-certificates

FROM scratch
ARG TARGETARCH
COPY --from=certs /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
COPY dist/linux/${TARGETARCH}/nasiko-docs-agent /agent
EXPOSE 8000
ENTRYPOINT ["/agent"]
