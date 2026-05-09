import logging

import docker as docker_sdk
from docker.errors import NotFound, APIError

logger = logging.getLogger(__name__)

_client: docker_sdk.DockerClient | None = None


def _get_client() -> docker_sdk.DockerClient:
    global _client
    if _client is None:
        _client = docker_sdk.from_env()
    return _client


def pause_container(container_name: str) -> None:
    try:
        container = _get_client().containers.get(container_name)
        container.pause()
        logger.info(f"paused container: {container_name}")
    except NotFound:
        raise ValueError(f"container not found: {container_name}")
    except APIError as exc:
        raise RuntimeError(f"docker pause failed: {exc}")


def unpause_container(container_name: str) -> None:
    try:
        container = _get_client().containers.get(container_name)
        container.unpause()
        logger.info(f"unpaused container: {container_name}")
    except NotFound:
        raise ValueError(f"container not found: {container_name}")
    except APIError as exc:
        raise RuntimeError(f"docker unpause failed: {exc}")


def run_replica(source_container_name: str, replica_name: str, network: str) -> str:
    try:
        source = _get_client().containers.get(source_container_name)
        image = source.image.tags[0] if source.image.tags else source.image.id
        env = source.attrs.get("Config", {}).get("Env", [])
        container = _get_client().containers.run(
            image=image,
            name=replica_name,
            environment=env,
            network=network,
            detach=True,
            remove=False,
        )
        logger.info(f"started replica {replica_name} from {source_container_name}")
        return container.id
    except NotFound:
        raise ValueError(f"source container not found: {source_container_name}")
    except APIError as exc:
        raise RuntimeError(f"docker run replica failed: {exc}")


def stop_container(container_name: str) -> None:
    try:
        container = _get_client().containers.get(container_name)
        container.stop(timeout=10)
        container.remove(force=True)
        logger.info(f"stopped and removed container: {container_name}")
    except NotFound:
        logger.warning(f"container not found on stop (already removed?): {container_name}")
    except APIError as exc:
        raise RuntimeError(f"docker stop failed: {exc}")


def wait_for_health(container_name: str, timeout_seconds: int = 60) -> bool:
    import time
    deadline = time.monotonic() + timeout_seconds
    client = _get_client()
    while time.monotonic() < deadline:
        try:
            container = client.containers.get(container_name)
            container.reload()
            health = container.attrs.get("State", {}).get("Health", {})
            status = health.get("Status", "")
            if status == "healthy":
                return True
            if container.status not in ("running", "starting"):
                return False
        except NotFound:
            return False
        time.sleep(2)
    return False
