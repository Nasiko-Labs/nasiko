import pytest


@pytest.fixture(autouse=True)
def _reset_env(monkeypatch):
    """Clear LITELLM_* env so each test starts from a known state."""
    for key in (
        "LITELLM_ENABLED",
        "LITELLM_BASE_URL",
        "LITELLM_VIRTUAL_KEY",
        "LITELLM_DEFAULT_MODEL",
    ):
        monkeypatch.delenv(key, raising=False)
    yield
