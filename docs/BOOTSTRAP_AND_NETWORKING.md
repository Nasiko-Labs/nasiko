# Bootstrap & Networking Architecture

## Overview

The Nasiko platform uses a two-phase deployment model:

1. **Phase 1 (CLI → CP):** The `nasiko` CLI on the user's machine directly calls cloud provider APIs to create a VPC and deploy the control plane VM.
2. **Phase 2 (CP → Pool):** The control plane uses Terraform (running internally) to create and manage the agent VM pool within the same VPC.

All infrastructure state lives on the control plane. The user's machine stores only minimal metadata for discovery and teardown.

## Design Principles

- **Single binary CP** — no Docker/containers on the CP VM; the server binary runs directly (the `nasiko-server` crate, bin `control_plane`)
- **Self-sufficient CP** — Postgres, Redis, and S3-compatible storage can be self-hosted on the CP VM OR use managed services provided by the user
- **CLI is lightweight** — no Terraform required on the user's machine; just the `nasiko` binary
- **Private networking** — CP and pool VMs share a VPC; agents are never publicly exposed
- **CP is the sole ingress** — only the CP has a public IP; it proxies all traffic to agents

## Supported Providers (Initial)

- **DigitalOcean**
- **AWS**

## Phase 1: CLI Bootstraps the Control Plane

### What the CLI does (4-5 cloud API calls)

```
$ nasiko init

? Cloud provider: aws
? Region: us-east-1
? Instance size: t3.medium
? SSH key: ~/.ssh/id_ed25519
? Managed Postgres URL (optional, Enter to self-host):
? Managed Redis URL (optional, Enter to self-host):
? S3 bucket for artifacts (optional, Enter to self-host):

[1/4] Creating VPC (10.0.0.0/16)...          ✓ vpc-abc123
[2/4] Creating subnet + security group...     ✓ subnet-def456
[3/4] Launching control plane VM...           ✓ i-789xyz (10.0.1.5)
[4/4] Waiting for CP to become healthy...     ✓ https://203.0.113.10/health

Control plane ready!
  Dashboard: https://203.0.113.10
  API:       https://203.0.113.10/api
```

> **Port note:** the server binds to `0.0.0.0:8080` by default, controlled by `CP_BIND`.
> The HTTPS/443 examples in this doc assume the CP is configured to serve TLS on 443
> (e.g. `CP_BIND=0.0.0.0:443`).

### Resources created per provider

| Resource | AWS | DigitalOcean |
|----------|-----|--------------|
| Network | VPC + Subnet | VPC |
| Firewall | Security Group | Firewall |
| VM | EC2 instance | Droplet |
| Public IP | Elastic IP | Reserved IP |
| SSH access | Key pair | SSH key |

### Security group / Firewall rules (CP VM)

| Port | Source | Purpose |
|------|--------|---------|
| 443 | `0.0.0.0/0` | HTTPS API + Dashboard (see port note above — default bind is `0.0.0.0:8080` via `CP_BIND`) |
| 22 | User's IP (or restricted CIDR) | SSH for maintenance |
| All | VPC CIDR | Internal communication with pool |

### Cloud-init on the CP VM

The CLI injects a cloud-init script that:

1. Downloads the server binary (`control_plane`, from GitHub release or pre-baked image)
2. Writes config to `/etc/nasiko/config.toml` with:
   - VPC/subnet IDs (for later pool creation)
   - Database URL (managed or `postgres://localhost/nasiko`)
   - Redis URL (managed or `redis://127.0.0.1/`)
   - S3/storage config (managed bucket or local MinIO)
   - SSH key path for pool access
3. If self-hosting data services:
   - Installs Postgres 16 via apt
   - Installs Redis via apt
   - Installs MinIO as a systemd service (S3-compatible, local disk)
4. Runs DB migrations
5. Starts the `control_plane` binary as a systemd service
6. Generates TLS cert (Let's Encrypt via ACME, or self-signed initially)

### Local metadata stored by CLI

```
~/.nasiko/clusters.json
```

```json
{
  "clusters": [
    {
      "name": "production",
      "provider": "aws",
      "region": "us-east-1",
      "vpc_id": "vpc-abc123",
      "cp_instance_id": "i-789xyz",
      "cp_public_ip": "203.0.113.10",
      "cp_private_ip": "10.0.1.5",
      "created_at": "2026-05-28T10:00:00Z"
    }
  ]
}
```

This is just enough to find the CP for health checks and teardown. All real state lives on the CP.

## Phase 2: CP Creates the Agent Pool

Once the CP is running, the user (via dashboard or API) triggers pool creation:

```
POST /api/pool/create
{
  "min_nodes": 1,
  "max_nodes": 5,
  "instance_size": "t3.small"
}
```

The CP internally:

1. Writes terraform variables (VPC ID, subnet ID, instance size, pool size, CP private IP)
2. Runs `terraform init && terraform apply`
3. Terraform creates VMs in the same VPC/subnet
4. Cloud-init on pool VMs installs Docker + creates `nasiko-agents` network
5. Pool VMs register with CP via private IP: `POST http://{cp_private_ip}:8080/nodes/{node_id}/ready`
6. Nodes become available for container deployment

### Pool VM security group / Firewall rules

| Port | Source | Purpose |
|------|--------|---------|
| 22 | CP private IP only | SSH for container management |
| 8000 | VPC CIDR only | Agent container port (all agents standardized to 8000) |

**Agents are never publicly exposed.** All external traffic flows through the CP's A2A proxy.

### Terraform state storage

Terraform state for the pool is stored in **Postgres** (on the CP) using the `pg` backend:

```hcl
terraform {
  backend "pg" {
    conn_str = "postgres://nasiko:pass@localhost/nasiko?sslmode=disable"
  }
}
```

This means:
- State survives CP process restarts
- State is backed up with regular Postgres backups
- No external S3 bucket needed just for terraform state
- If CP VM disk dies, state is recoverable from Postgres backups

## Network Topology

```
              ┌─── Public Internet ───┐
              │                       │
              ▼                       │
    ┌─────────────────┐              │
    │  Control Plane  │              │
    │  203.0.113.10   │  (public)    │
    │  10.0.1.5       │  (private)   │
    │  ─────────────  │              │
    │  nasiko-server  │              │
    │  Postgres       │  (or managed)│
    │  Redis          │  (or managed)│
    │  MinIO          │  (or managed S3)
    └──┬─────┬────────┘
       │     │
  [SSH:22]  [HTTP:8000]    ← private network (or Docker compose network in local mode)
       │     │
    ┌──▼─────▼────┐    ┌──────────────┐
    │  Pool VM 1  │    │  Pool VM 2   │
    │  10.0.2.10  │    │  10.0.2.11   │
    │  Docker     │    │  Docker      │
    │  Agent A    │    │  Agent B     │
    │  Agent C    │    │  Agent D     │
    └─────────────┘    └──────────────┘
```

**Traffic flow:**
- External client → CP (public IP; port 443 when TLS is configured, otherwise the `CP_BIND` port, default 8080)
- CP A2A proxy → Agent container (via Docker network name or private IP, port 8000)
- CP orchestrator → Pool VM (SSH over private IP)
- CP health checks → Agent container (HTTP over private IP or container name)
- Pool VM registration → CP (HTTP over private IP)

**Local mode (Docker Compose):** The local orchestrator attaches agent containers to the compose network (`nasiko_default`). The CP reaches agents by container name (e.g. `http://echo-agent:8000`). Agent URLs are stored in the database and used by the proxy directly.

## Self-Hosted vs Managed Services

The CP supports both modes, configured at bootstrap time:

| Service | Self-hosted (default) | Managed (user provides) |
|---------|----------------------|------------------------|
| **Postgres** | Installed on CP VM via apt, data on local disk | Any Postgres URL (RDS, Cloud SQL, DO Managed DB) |
| **Redis** | Installed on CP VM via apt | Any Redis URL (ElastiCache, Memorystore, DO Redis) |
| **Object Storage** | MinIO on CP VM, local disk | S3, GCS, DO Spaces (any S3-compatible endpoint) |

### When to use managed

- **Production with HA requirements** — managed services provide backups, failover, replication
- **Large deployments** — CP VM disk may not be enough for Postgres + MinIO + binary
- **Compliance** — managed services often have encryption-at-rest, audit logs

### When self-hosted is fine

- **Development / small teams** — single VM with everything is simplest
- **Cost-sensitive** — one VM is cheaper than VM + managed DB + managed Redis + S3
- **Air-gapped / on-prem** — no cloud services available

## Teardown

```
$ nasiko destroy

? This will delete the control plane and all agent pools. Continue? Yes

[1/3] Destroying agent pool...                ✓ (CP runs terraform destroy)
[2/3] Terminating control plane VM...         ✓
[3/3] Deleting VPC + networking...            ✓

All resources cleaned up.
```

The CLI:
1. Calls CP API to destroy pool (CP runs `terraform destroy` internally)
2. Waits for pool teardown confirmation
3. Terminates the CP VM via cloud API
4. Deletes VPC, subnet, security group, elastic IP
5. Removes entry from `~/.nasiko/clusters.json`

## CLI Implementation Notes

### Cloud API calls (no Terraform on user machine)

**AWS** — use `aws-sdk-ec2` (Rust):
- `CreateVpc`, `CreateSubnet`, `CreateSecurityGroup`, `AuthorizeSecurityGroupIngress`
- `RunInstances` (with UserData for cloud-init)
- `AllocateAddress`, `AssociateAddress`

**DigitalOcean** — use REST API via `reqwest`:
- `POST /v2/vpcs` (create VPC)
- `POST /v2/firewalls` (create firewall)
- `POST /v2/droplets` (create droplet with vpc_uuid + user_data)
- `POST /v2/reserved_ips` (create and assign reserved IP)

### Authentication

The CLI reads credentials from standard locations:
- **AWS:** `~/.aws/credentials` or env vars (`AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`)
- **DigitalOcean:** `DIGITALOCEAN_TOKEN` env var or `~/.config/doctl/config.yaml`

### CP Health Check

After launching the VM, the CLI polls `https://{public_ip}/health` with exponential backoff (max 5 minutes). Cloud-init typically takes 1-3 minutes.

## Future Considerations

- **Multi-region pools:** CP in one region, pool VMs in another (requires VPC peering)
- **CP high availability:** Multiple CP instances behind a load balancer (stateless — shared Postgres + Redis)
- **Managed Kubernetes option:** For users who want K8s instead of bare VMs — a Kubernetes agent runtime is available in the Nasiko enterprise edition
- **Custom domains:** Let's Encrypt ACME with DNS challenge for `cp.example.com`
- **Backup/restore:** CLI command to snapshot CP state (Postgres dump + terraform state + config)
