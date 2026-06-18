FROM scratch
COPY target/x86_64-unknown-linux-musl/release/nasiko-nutrition-agent /agent
EXPOSE 8000
ENTRYPOINT ["/agent"]
