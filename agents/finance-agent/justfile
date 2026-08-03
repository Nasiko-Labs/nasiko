user := env("DOCKERHUB_USER", "akhilfolium")
docker := env("DOCKER", if path_exists("/usr/bin/podman") == "true" { "podman" } else { "docker" })

build:
    {{docker}} build -t {{user}}/finance-agent .

push:
    {{docker}} push {{user}}/finance-agent

release: build push
