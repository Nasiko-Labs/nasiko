import importlib.util
from pathlib import Path
from types import SimpleNamespace


def _load_registry_module():
    repo_root = Path(__file__).resolve().parents[1]
    module_path = repo_root / "agent-gateway" / "registry" / "registry.py"
    spec = importlib.util.spec_from_file_location("kong_registry_module", module_path)
    module = importlib.util.module_from_spec(spec)
    assert spec and spec.loader
    spec.loader.exec_module(module)
    return module


def test_build_route_path_agent_unchanged_and_mcp_added():
    mod = _load_registry_module()

    assert mod._build_route_path("agent-foo", "agent") == "/agents/agent-foo"
    assert mod._build_route_path("mcp-foo", "mcp_server") == "/mcp/mcp-foo"


def test_infer_artifact_type_from_service_name_and_env():
    mod = _load_registry_module()

    assert mod._infer_artifact_type("mcp-abc", "nasiko-agents") == "mcp_server"
    assert mod._infer_artifact_type("agent-abc", "nasiko-agents") == "agent"
    assert (
        mod._infer_artifact_type(
            "agent-abc",
            "docker-agents",
            env_map={"ARTIFACT_TYPE": "mcp_server"},
        )
        == "mcp_server"
    )


def test_docker_discovery_adds_mcp_route_only_for_mcp_artifacts(monkeypatch):
    mod = _load_registry_module()

    class _FakeContainer:
        def __init__(self, name, env):
            self.name = name
            self.status = "running"
            self.attrs = {"Config": {"Env": env}}

        def reload(self):
            return None

    class _FakeNetwork:
        containers = [
            _FakeContainer("agent-normal", ["ARTIFACT_TYPE=agent"]),
            _FakeContainer("agent-mcp", ["ARTIFACT_TYPE=mcp_server"]),
        ]

    class _FakeDockerClient:
        class networks:
            @staticmethod
            def get(_):
                return _FakeNetwork()

    monkeypatch.setattr(mod, "K8S_ENABLED", False)
    monkeypatch.setattr(mod, "get_docker_client", lambda: _FakeDockerClient())

    services = mod.get_docker_services()
    routes = {svc.name: svc.path for svc in services}

    assert routes["agent-normal"] == "/agents/agent-normal"
    assert routes["agent-mcp"] == "/mcp/agent-mcp"


def test_route_naming_remains_stable_for_mcp_registration(monkeypatch):
    mod = _load_registry_module()

    captured = {"route_name": None, "service_name": None}

    class _Resp:
        def __init__(self, status_code=200, payload=None, text=""):
            self.status_code = status_code
            self._payload = payload or {}
            self.text = text

        def json(self):
            return self._payload

    def _fake_get(url, timeout=None):
        # Service/route not found -> trigger create path
        if "/services/" in url or "/routes/" in url:
            return _Resp(status_code=404)
        return _Resp(status_code=200, payload={"data": []})

    def _fake_post(url, json=None, timeout=None):
        if "/services" in url and "/routes" not in url:
            captured["service_name"] = json["name"]
            return _Resp(status_code=201)
        if "/routes" in url:
            captured["route_name"] = json["name"]
            return _Resp(status_code=201)
        if "/plugins" in url:
            return _Resp(status_code=201)
        return _Resp(status_code=201)

    monkeypatch.setattr(mod.requests, "get", _fake_get)
    monkeypatch.setattr(mod.requests, "post", _fake_post)
    monkeypatch.setattr(mod.requests, "patch", lambda *a, **k: _Resp(status_code=200))

    svc = mod.ServiceInfo(
        name="mcp-demo",
        host="mcp-demo.local",
        port=8080,
        path="/mcp/mcp-demo",
        methods=["GET", "POST"],
        namespace="nasiko-agents",
    )

    ok = mod.register_service_in_kong(svc)
    assert ok is True
    assert captured["service_name"] == "mcp-demo"
    assert captured["route_name"] == "mcp-demo-route"
