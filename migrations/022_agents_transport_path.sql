-- `url` stores the runtime-internal endpoint (container IP / service DNS) that
-- the gateway proxies TO — nasiko_agent_proxy::resolve() parses host:port from
-- it and deliberately ignores any path. Clients additionally need the path the
-- agent advertises for its JSON-RPC transport in its own AgentCard
-- (supportedInterfaces[].url, e.g. "/jsonrpc") to build a working proxy URL:
--   {base}/api/agents/{id}{transport_path}
-- The A2A spec fixes no path, so it must be discovered from the card and
-- persisted here at deploy time rather than guessed (or re-fetched) by clients.
ALTER TABLE agents ADD COLUMN IF NOT EXISTS transport_path TEXT;
