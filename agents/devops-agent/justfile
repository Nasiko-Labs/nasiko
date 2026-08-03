user := env("DOCKERHUB_USER", "akhilfolium")
docker := env("DOCKER", if path_exists("/usr/bin/podman") == "true" { "podman" } else { "docker" })

build:
    {{docker}} build -t {{user}}/devops-agent .

push:
    {{docker}} push {{user}}/devops-agent

release: build push
