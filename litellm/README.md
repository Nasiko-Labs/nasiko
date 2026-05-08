# LiteLLM Gateway Configuration

This directory holds the LiteLLM proxy configuration used by the platform-managed
LLM gateway introduced in Track 2 of the Nasiko hackathon contribution.

- `config.yaml` — model routing (`model_name` → upstream provider + key)

The gateway is deployed as a first-class service in `docker-compose.local.yml`.
Agents receive `LITELLM_BASE_URL` and a virtual key (`LITELLM_VIRTUAL_KEY`) via
env injection at deploy time and no longer need provider keys baked into their
source.

See [`../docs/LLM_GATEWAY.md`](../docs/LLM_GATEWAY.md) for the full design,
usage, and the virtual-key provisioning model.
