"""
CLI documentation renderer for `nasiko docs`.
Displays rich terminal documentation for all CLI commands.
"""

from rich.console import Console
from rich.panel import Panel
from rich.table import Table
from rich.rule import Rule
from rich.columns import Columns
from rich.text import Text
from rich import box

console = Console()


# ─────────────────────────────────────────────────────────
# Helpers
# ─────────────────────────────────────────────────────────

def _header(title: str, subtitle: str = ""):
    console.print()
    console.print(Panel(
        f"[bold white]{title}[/bold white]\n[dim]{subtitle}[/dim]" if subtitle else f"[bold white]{title}[/bold white]",
        border_style="bright_blue",
        padding=(0, 2),
    ))


def _section(title: str):
    console.print()
    console.print(Rule(f"[bold cyan]{title}[/bold cyan]", style="dim"))
    console.print()


def _cmd_table(rows: list[tuple], headers=("Command", "Description")):
    """Render a command table. rows = [(cmd, desc), ...]"""
    t = Table(box=box.SIMPLE_HEAD, show_header=True, header_style="bold magenta",
              show_edge=False, pad_edge=False)
    for h in headers:
        t.add_column(h, style="cyan" if h == headers[0] else "white")
    for row in rows:
        t.add_row(*row)
    console.print(t)


def _flags_table(rows: list[tuple]):
    """rows = [(flag, short, required/default, description)]"""
    t = Table(box=box.SIMPLE_HEAD, show_header=True, header_style="bold magenta",
              show_edge=False, pad_edge=False)
    t.add_column("Flag", style="cyan", no_wrap=True)
    t.add_column("Short", style="yellow", no_wrap=True)
    t.add_column("Default/Required", no_wrap=True)
    t.add_column("Description", style="white")
    for flag, short, req, desc in rows:
        req_styled = (
            "[bold red]required[/bold red]" if req == "required"
            else f"[dim]{req}[/dim]"
        )
        t.add_row(flag, short, req_styled, desc)
    console.print(t)


def _code(text: str):
    console.print(Panel(text, border_style="dim", padding=(0, 2)))


def _tip(text: str):
    console.print(f"\n[bold yellow]💡  Tip:[/bold yellow] {text}\n")


# ─────────────────────────────────────────────────────────
# OVERVIEW
# ─────────────────────────────────────────────────────────

def show_overview():
    console.print(Panel(
        "[bold bright_cyan]Nasiko CLI  v2.0.0[/bold bright_cyan]\n\n"
        "Build, deploy, and manage AI agents from your terminal.\n"
        "Run agents locally with Docker or deploy to AWS / DigitalOcean\n"
        "with a single command.\n\n"
        "[dim]Think of it like[/dim] [bold]git[/bold][dim] — you have clusters (remotes)\n"
        "and an active cluster (checked-out branch).[/dim]",
        title="[bold]nasiko docs[/bold]",
        border_style="bright_cyan",
        padding=(1, 4),
    ))

    _section("Command Groups")
    _cmd_table([
        ("nasiko init",          "First-run wizard — create cluster, set active, log in"),
        ("nasiko use <cluster>", "Switch active cluster"),
        ("nasiko current",       "Show active cluster + auth state"),
        ("nasiko auth",          "Login, logout, status"),
        ("nasiko cluster",       "Create, connect, destroy, inspect clusters"),
        ("nasiko github",        "Connect GitHub, clone repos as agents"),
        ("nasiko agent",         "Deploy and manage AI agents"),
        ("nasiko chat",          "Create sessions and chat with agents"),
        ("nasiko observability", "Sessions, traces, spans, stats"),
        ("nasiko access",        "Grant / revoke agent permissions"),
        ("nasiko local",         "Manage local Docker Compose stack"),
        ("nasiko n8n",           "Connect N8N and register workflows as agents"),
        ("nasiko images",        "Build and push Nasiko core images"),
        ("nasiko user",          "User management (super user only)"),
        ("nasiko search",        "Search users and agents"),
    ])

    _section("Drill into a topic")
    console.print("[dim]Usage:[/dim]  [cyan]nasiko docs <topic>[/cyan]\n")
    topics = [
        "install", "quickstart", "concepts",
        "auth", "cluster", "github", "agent",
        "chat", "observability", "access", "local",
        "n8n", "images", "user", "search", "env",
    ]
    console.print("  " + "  ".join(f"[cyan]{t}[/cyan]" for t in topics))
    console.print()


# ─────────────────────────────────────────────────────────
# INSTALL
# ─────────────────────────────────────────────────────────

def show_install():
    _header("Installation", "Get the Nasiko CLI on your machine")

    _section("Install via pip")
    _code("pip install nasiko-cli")

    _section("Recommended — use a virtual environment")
    _code(
        "python -m venv .venv\n"
        "source .venv/bin/activate     # macOS / Linux\n"
        ".venv\\Scripts\\activate        # Windows\n"
        "pip install nasiko-cli"
    )

    _section("Verify")
    _code("nasiko --version\n# Nasiko CLI v2.0.0")

    _section("Prerequisites")

    console.print("[bold]All setups[/bold]")
    _flags_table([
        ("Python", "", "3.10+", "Runtime for the CLI"),
        ("pip",    "", "any",   "Package manager"),
    ])

    console.print("\n[bold]Local setup (Docker Compose)[/bold]")
    _flags_table([
        ("Docker Desktop / Docker Engine", "", "required", "docker --version"),
        ("Docker Compose v2",              "", "required", "docker compose version"),
    ])

    console.print("\n[bold]Remote setup (AWS / DigitalOcean)[/bold]")
    _flags_table([
        ("terraform", "", "≥ 1.3",    "terraform -v"),
        ("kubectl",   "", "required", "kubectl version --client"),
        ("AWS or DO credentials", "", "required", "See nasiko docs env"),
    ])


# ─────────────────────────────────────────────────────────
# QUICK START
# ─────────────────────────────────────────────────────────

def show_quickstart():
    _header("Quick Start", "From zero to running agents")

    _section("Option A — Local (Docker Compose)  [recommended for first-timers]")
    steps = [
        ("1", "Run the setup wizard",
         "nasiko init\n"
         "  # Cluster type: local\n"
         "  # Cluster name: local  (default)"),
        ("2", "Find your credentials",
         "cat orchestrator/superuser_credentials.json"),
        ("3", "Log in",
         "nasiko auth login\n"
         "  # Access Key:    NASK_xxxx\n"
         "  # Access Secret: ****"),
        ("4", "Deploy your first agent",
         "nasiko agent upload-directory ./my-agent"),
    ]
    for num, title, code in steps:
        console.print(f" [bold bright_cyan]{num}.[/bold bright_cyan] [bold]{title}[/bold]")
        _code(code)

    _section("Option B — Remote (AWS)")
    steps_b = [
        ("1", "Set AWS credentials",
         "export AWS_ACCESS_KEY_ID=AKIA...\n"
         "export AWS_SECRET_ACCESS_KEY=...\n"
         "export AWS_DEFAULT_REGION=us-east-1"),
        ("2", "Run the wizard",
         "nasiko init\n"
         "  # Cluster type: remote\n"
         "  # Cloud provider: aws\n"
         "  # Cluster name: my-nasiko\n"
         "  # Region: us-east-1"),
        ("3", "Confirm it's up",
         "nasiko current"),
    ]
    for num, title, code in steps_b:
        console.print(f" [bold bright_cyan]{num}.[/bold bright_cyan] [bold]{title}[/bold]")
        _code(code)

    _section("Option C — Connect to existing cluster")
    _code(
        "nasiko cluster connect prod --url https://api.my-cluster.example.com\n"
        "nasiko auth login"
    )


# ─────────────────────────────────────────────────────────
# CONCEPTS
# ─────────────────────────────────────────────────────────

def show_concepts():
    _header("Core Concepts", "How the CLI is designed")

    console.print(
        "\n[bold]Active cluster[/bold]\n"
        "Every command targets the active cluster. Set it with [cyan]nasiko use <name>[/cyan].\n"
        "Override for a single command with [cyan]--cluster / -n[/cyan].\n"
    )
    _code(
        "nasiko use local\n"
        "nasiko agent list           # runs on local\n"
        "nasiko agent list -n prod   # runs on prod without switching"
    )

    console.print(
        "\n[bold]Auth is per-cluster[/bold]\n"
        "Logging into [cyan]local[/cyan] does not log you into [cyan]prod-aws[/cyan].\n"
        "Each cluster has its own auth session.\n"
    )

    console.print(
        "\n[bold]State is stored locally[/bold]\n"
        "  [dim]~/.nasiko/context.json[/dim]       — active cluster\n"
        "  [dim]~/.nasiko/state/<provider>/<name>/[/dim]  — cluster info, Terraform state, credentials\n"
    )

    _section("Config file auto-detection (first match wins)")
    _cmd_table([
        (".nasiko-local.env", "Local overrides — highest priority"),
        (".nasiko.env",       "Project config"),
        (".nasiko-aws.env",   "AWS-specific config"),
        (".nasiko-do.env",    "DigitalOcean-specific config"),
        (".env",              "Generic fallback"),
    ], ("File", "Purpose"))


# ─────────────────────────────────────────────────────────
# AUTH
# ─────────────────────────────────────────────────────────

def show_auth():
    _header("nasiko auth", "Authentication — per-cluster")

    _section("Commands")
    _cmd_table([
        ("nasiko auth login",   "Authenticate to the active cluster. Prompts if no flags given."),
        ("nasiko auth logout",  "Clear your session for the active cluster."),
        ("nasiko auth status",  "Show auth state, user info, and API connectivity."),
    ])

    _section("nasiko auth login — flags")
    _flags_table([
        ("--access-key",    "-k", "required", "Access key (starts with NASK_)"),
        ("--access-secret", "-s", "required", "Access secret (hidden input)"),
        ("--api-url",       "",   "optional", "Override the API URL for this login"),
    ])

    _section("Example")
    _code(
        "nasiko auth login\n"
        "nasiko auth login -k NASK_xxx -s mysecret\n"
        "nasiko auth status"
    )


# ─────────────────────────────────────────────────────────
# CLUSTER
# ─────────────────────────────────────────────────────────

def show_cluster():
    _header("nasiko cluster", "Cluster lifecycle — create, connect, inspect, destroy")

    _section("Commands overview")
    _cmd_table([
        ("nasiko cluster connect <name> --url <url>", "Register an existing cluster by API URL"),
        ("nasiko cluster create local",               "Start Docker Compose, register as local cluster"),
        ("nasiko cluster create remote",              "Provision K8s on AWS/DO via Terraform"),
        ("nasiko cluster bootstrap",                  "All-in-one: k8s + registry + buildkit + core"),
        ("nasiko cluster list",                       "List all clusters in local state"),
        ("nasiko cluster destroy <name>",             "Tear down a cluster"),
        ("nasiko cluster output",                     "Show Terraform outputs (IPs, kubeconfig)"),
        ("nasiko cluster state-info",                 "Terraform state details"),
        ("nasiko cluster init-modules",               "Copy Terraform modules to ~/.nasiko/terraform/"),
        ("nasiko cluster setup registry harbor",      "Deploy Harbor container registry via Helm"),
        ("nasiko cluster setup registry cloud",       "Setup ECR (AWS) or DO container registry"),
        ("nasiko cluster setup buildkit",             "Deploy rootless BuildKit"),
        ("nasiko cluster setup core",                 "Deploy backend, web, router, auth + infra"),
        ("nasiko cluster configure-github-oauth",     "Patch GitHub OAuth vars without re-bootstrap"),
        ("nasiko cluster cleanup",                    "Remove all Nasiko resources from cluster"),
        ("nasiko cluster init-superuser",             "Create/recreate superuser credentials"),
        ("nasiko cluster get-superuser",              "Fetch existing superuser credentials"),
    ])

    _section("nasiko cluster connect — flags")
    _flags_table([
        ("<name>",             "",   "required", "Name to give this cluster locally"),
        ("--url",              "-u", "required", "API gateway URL (https://...)"),
        ("--login/--no-login", "",   "true",     "Prompt for login after connecting"),
    ])

    _section("nasiko cluster create remote — flags")
    _flags_table([
        ("--provider",      "",   "required",          "aws or digitalocean"),
        ("--name",          "-n", "nasiko",             "Cluster name"),
        ("--region",        "",   "optional",           "e.g. us-east-1, nyc3"),
        ("--node-size",     "",   "optional",           "e.g. t3.medium, s-2vcpu-4gb"),
        ("--yes",           "-y", "false",              "Auto-approve Terraform apply"),
        ("--verbose",       "-v", "false",              "Show full Terraform output"),
        ("--terraform-dir", "-t", "~/.nasiko/terraform","Terraform modules path"),
        ("--state-dir",     "-s", "~/.nasiko/state",    "Terraform state path"),
    ])

    _section("Example")
    _code(
        "# Connect to existing cluster\n"
        "nasiko cluster connect prod --url https://api.my-cluster.example.com\n\n"
        "# Create local Docker stack\n"
        "nasiko cluster create local\n\n"
        "# Provision on AWS\n"
        "nasiko cluster create remote --provider aws --name prod --region us-east-1\n\n"
        "# All-in-one\n"
        "nasiko cluster bootstrap --provider aws --cluster-name prod"
    )


# ─────────────────────────────────────────────────────────
# GITHUB
# ─────────────────────────────────────────────────────────

def show_github():
    _header("nasiko github", "GitHub integration — per-cluster")

    console.print(
        "\n[bold yellow]⚠  Prerequisite:[/bold yellow] GitHub integration requires a GitHub OAuth App\n"
        "   configured on the cluster. Run [cyan]nasiko github connect[/cyan] — if it's not\n"
        "   configured yet, you'll see step-by-step setup instructions.\n"
    )

    _section("Commands")
    _cmd_table([
        ("nasiko github connect",        "Link GitHub. Prints status if already connected. Guides setup if not configured."),
        ("nasiko github disconnect",     "Unlink GitHub from this cluster"),
        ("nasiko github status",         "Show GitHub connection state"),
        ("nasiko github repos",          "List your accessible GitHub repositories"),
        ("nasiko github clone [repo]",   "Clone a repo and deploy it as an agent (interactive if repo omitted)"),
    ])

    _section("nasiko github clone — flags")
    _flags_table([
        ("[repo]", "",   "optional", "owner/repo or full GitHub URL. Shows list if omitted."),
        ("--branch", "-b", "main",  "Branch to clone"),
    ])

    _section("Example")
    _code(
        "nasiko github connect\n"
        "nasiko github repos\n"
        "nasiko github clone owner/my-agent-repo\n"
        "nasiko github clone                    # interactive list"
    )

    _section("GitHub OAuth setup steps")
    steps = [
        "Go to [cyan]https://github.com/settings/developers[/cyan] → OAuth Apps → New OAuth App",
        "Set Homepage URL and Callback URL to your cluster's API URL\n"
        "   e.g. Homepage: [cyan]http://localhost:9100[/cyan]   Callback: [cyan]http://localhost:9100/auth/github/callback[/cyan]",
        "Add to your env file:\n   GITHUB_CLIENT_ID=your_id\n   GITHUB_CLIENT_SECRET=your_secret",
        "Restart the stack:  [cyan]nasiko local down && nasiko local up[/cyan]",
    ]
    for i, s in enumerate(steps, 1):
        console.print(f" [bold bright_cyan]{i}.[/bold bright_cyan] {s}\n")


# ─────────────────────────────────────────────────────────
# AGENT
# ─────────────────────────────────────────────────────────

def show_agent():
    _header("nasiko agent", "Deploy and manage AI agents")

    _section("Commands")
    _cmd_table([
        ("nasiko agent upload-directory <path>", "Deploy an agent from a local directory"),
        ("nasiko agent upload-zip <file>",       "Deploy an agent from a .zip file"),
        ("nasiko agent list",                    "List all agents in the registry"),
        ("nasiko agent list-uploaded",           "List agents uploaded by the current user"),
        ("nasiko agent get",                     "Get detailed info about a specific agent"),
    ])

    _section("nasiko agent list — flags")
    _flags_table([
        ("--format", "-f", "table", "table, json, or list"),
        ("--details", "-d", "false", "Show additional details"),
    ])

    _section("nasiko agent get — flags")
    _flags_table([
        ("--agent-id", "",   "optional", "Look up by agent ID (one of --agent-id or --name required)"),
        ("--name",     "",   "optional", "Look up by agent name"),
        ("--format",   "-f", "details",  "details or json"),
    ])

    _section("Example")
    _code(
        "nasiko agent upload-directory ./agents/my-agent\n"
        "nasiko agent list\n"
        "nasiko agent list --format json\n"
        "nasiko agent get --name my-agent"
    )


# ─────────────────────────────────────────────────────────
# CHAT
# ─────────────────────────────────────────────────────────

def show_chat():
    _header("nasiko chat", "Chat sessions with deployed agents")

    _section("Commands")
    _cmd_table([
        ("nasiko chat create-session",          "Create a new chat session (--agent/-a for agent name)"),
        ("nasiko chat list-sessions",           "List sessions (--limit/-l, --cursor, --direction)"),
        ("nasiko chat history <session_id>",    "View conversation history"),
        ("nasiko chat delete-session <id>",     "Delete a session"),
        ("nasiko chat send",                    "Send a one-shot message (--url/-u, --session-id/-s, --message/-m)"),
    ])


# ─────────────────────────────────────────────────────────
# OBSERVABILITY
# ─────────────────────────────────────────────────────────

def show_observability():
    _header("nasiko observability", "Monitor sessions, traces, and agent performance")

    _section("Commands")
    _cmd_table([
        ("nasiko observability sessions [agent_id]",         "Recent sessions, optionally filtered by agent"),
        ("nasiko observability session <session_id>",        "Full details for one session"),
        ("nasiko observability trace <project_id> <trace_id>", "Trace with nested spans"),
        ("nasiko observability span <span_id>",              "Individual span details"),
        ("nasiko observability stats <agent_id>",            "Performance stats for an agent"),
    ])

    _section("Common flags")
    _flags_table([
        ("--days",   "-d", "7",       "Days to look back"),
        ("--limit",  "-l", "20",      "Max results"),
        ("--format", "-f", "table",   "table, json, summary, detailed, tree, spans, traces"),
    ])


# ─────────────────────────────────────────────────────────
# ACCESS
# ─────────────────────────────────────────────────────────

def show_access():
    _header("nasiko access", "Control who can access an agent")

    _section("Commands")
    _cmd_table([
        ("nasiko access grant-user <agent_id>",   "Grant user(s) access  (--user-id/-u, repeatable)"),
        ("nasiko access grant-agent <agent_id>",  "Grant agent-to-agent access  (--agent-id/-a, repeatable)"),
        ("nasiko access list <agent_id>",         "Show current permissions"),
        ("nasiko access revoke-user <agent_id>",  "Revoke user access"),
        ("nasiko access revoke-agent <agent_id>", "Revoke agent-to-agent access"),
    ])


# ─────────────────────────────────────────────────────────
# LOCAL
# ─────────────────────────────────────────────────────────

def show_local():
    _header("nasiko local", "Manage the local Docker Compose development stack")

    _section("Commands")
    _cmd_table([
        ("nasiko local up",                      "Start the full stack. Checks port conflicts first."),
        ("nasiko local down",                    "Stop and remove the stack (--volumes/-v deletes all data)"),
        ("nasiko local status",                  "Show running containers"),
        ("nasiko local logs [service...]",       "View logs (--follow/-f, --lines/-n 100)"),
        ("nasiko local restart [service]",       "Restart one or all services"),
        ("nasiko local shell <service>",         "Open bash in a running container"),
        ("nasiko local deploy-agent <name> [path]", "Deploy agent to local stack via backend API"),
    ])

    _section("Services & ports")
    _cmd_table([
        ("Kong Gateway",     "9100  ← use as NASIKO_API_URL"),
        ("Backend API",      "8000  /docs for Swagger"),
        ("Auth Service",     "8082"),
        ("Router",           "8081"),
        ("Chat History",     "8083"),
        ("Web UI",           "4000"),
        ("Konga (Kong UI)",  "1337"),
        ("MongoDB",          "27017"),
        ("Redis",            "6379"),
    ], ("Service", "Port / Notes"))

    _tip("All ports can be overridden via NASIKO_PORT_* env vars. Run [cyan]nasiko docs env[/cyan] for the full list.")


# ─────────────────────────────────────────────────────────
# N8N
# ─────────────────────────────────────────────────────────

def show_n8n():
    _header("nasiko n8n", "Connect N8N and register workflows as agents")

    _section("Commands")
    _cmd_table([
        ("nasiko n8n connect",              "Save N8N credentials (--url, --api-key, --connection-name — all required)"),
        ("nasiko n8n credentials",          "View saved N8N credentials"),
        ("nasiko n8n update",               "Update credentials (--name, --url, --api-key, --active)"),
        ("nasiko n8n delete",               "Remove credentials permanently"),
        ("nasiko n8n workflows",            "List workflows (--active-only, --limit/-l 100)"),
        ("nasiko n8n register <workflow_id>", "Register workflow as agent (--name/-n, --description/-d)"),
    ])

    _section("Example flow")
    _code(
        "nasiko n8n connect --url http://n8n.example.com --api-key <key> --connection-name prod\n"
        "nasiko n8n workflows              # find your workflow ID\n"
        "nasiko n8n register <id> --name my-workflow-agent"
    )


# ─────────────────────────────────────────────────────────
# IMAGES
# ─────────────────────────────────────────────────────────

def show_images():
    _header("nasiko images", "Build and push Nasiko core Docker images")

    _section("Commands")
    _cmd_table([
        ("nasiko images list",       "List all 9 core services with their Dockerfiles"),
        ("nasiko images build",      "Build images locally"),
        ("nasiko images push",       "Push images to a registry"),
        ("nasiko images build-push", "Build and push in one command"),
    ])

    _section("Shared flags (build / push / build-push)")
    _flags_table([
        ("--username",       "-u", "karannasiko",  "Registry namespace (Docker Hub)"),
        ("--tag",            "-t", "latest",       "Image tag"),
        ("--service",        "-s", "all services", "Specific service(s), repeatable"),
        ("--platform",       "",   "linux/amd64",  "Target platform(s)"),
        ("--multi-platform", "",   "false",        "Build for amd64 + arm64"),
        ("--no-cache",       "",   "false",        "Build without Docker cache"),
        ("--dry-run",        "",   "false",        "Print commands without running"),
    ])


# ─────────────────────────────────────────────────────────
# USER
# ─────────────────────────────────────────────────────────

def show_user():
    _header("nasiko user", "User management — super user access required")

    _section("Commands")
    _cmd_table([
        ("nasiko user register",                     "Register a new user (--username/-u, --email/-e, --super-user/-s)"),
        ("nasiko user list",                         "List all users (--limit/-l, default 50)"),
        ("nasiko user get <user_id>",                "Get user details"),
        ("nasiko user regenerate-credentials <id>",  "Reset credentials for a user"),
        ("nasiko user revoke <user_id>",             "Revoke all tokens"),
        ("nasiko user reinstate <user_id>",          "Re-enable a revoked user"),
        ("nasiko user delete <user_id>",             "Permanently delete a user (--confirm skips prompt)"),
    ])


# ─────────────────────────────────────────────────────────
# SEARCH
# ─────────────────────────────────────────────────────────

def show_search():
    _header("nasiko search", "Search users and agents")

    _section("Commands")
    _cmd_table([
        ("nasiko search users <query>",  "Search users (min 2 chars, --limit/-l max 50)"),
        ("nasiko search agents <query>", "Search agents (same flags)"),
    ])


# ─────────────────────────────────────────────────────────
# ENV VARS
# ─────────────────────────────────────────────────────────

def show_env():
    _header("Environment Variables", "Configure the CLI without flags")

    _section("Core")
    _cmd_table([
        ("NASIKO_API_URL",        "Override the API base URL (bypasses cluster lookup)"),
        ("NASIKO_CLUSTER_NAME",   "Active cluster name (set by nasiko use or --cluster/-n)"),
    ], ("Variable", "Description"))

    _section("Cloud credentials")
    _cmd_table([
        ("AWS_ACCESS_KEY_ID",          "AWS access key"),
        ("AWS_SECRET_ACCESS_KEY",      "AWS secret key"),
        ("AWS_DEFAULT_REGION",         "AWS default region"),
        ("DIGITALOCEAN_ACCESS_TOKEN",  "DigitalOcean API token"),
    ], ("Variable", "Description"))

    _section("Cluster setup")
    _cmd_table([
        ("NASIKO_PROVIDER",                  "aws or digitalocean"),
        ("NASIKO_REGION",                    "Cloud region (e.g. us-east-1, nyc3)"),
        ("KUBECONFIG",                       "Path to kubeconfig file"),
        ("NASIKO_CLUSTER_NAME",              "Cluster name"),
        ("NASIKO_TERRAFORM_DIR",             "Terraform modules directory"),
        ("NASIKO_STATE_DIR",                 "Terraform state directory"),
        ("NASIKO_TF_BACKEND",                "local, s3, gcs, or remote"),
    ], ("Variable", "Description"))

    _section("Registry & apps")
    _cmd_table([
        ("NASIKO_CONTAINER_REGISTRY_TYPE",  "harbor or cloud"),
        ("NASIKO_REGISTRY_USER",            "Registry username"),
        ("NASIKO_REGISTRY_PASS",            "Registry password"),
        ("NASIKO_DOMAIN",                   "Domain for Harbor"),
        ("NASIKO_EMAIL",                    "Email for SSL certificates"),
        ("NASIKO_SUPERUSER_USERNAME",       "Superuser username (default: admin)"),
        ("NASIKO_SUPERUSER_EMAIL",          "Superuser email"),
        ("OPENAI_API_KEY",                  "OpenAI key for LLM query routing"),
        ("NASIKO_PUBLIC_REGISTRY_USER",     "Docker Hub namespace for public images"),
    ], ("Variable", "Description"))

    _section("GitHub OAuth")
    _cmd_table([
        ("GITHUB_CLIENT_ID",     "GitHub OAuth App client ID"),
        ("GITHUB_CLIENT_SECRET", "GitHub OAuth App client secret"),
    ], ("Variable", "Description"))

    _section("Port overrides (local stack)")
    _cmd_table([
        ("NASIKO_PORT_KONG",             "9100 — Kong Gateway (main entry)"),
        ("NASIKO_PORT_KONG_ADMIN",       "9101 — Kong Admin API"),
        ("NASIKO_PORT_BACKEND",          "8000 — Backend API"),
        ("NASIKO_PORT_AUTH",             "8082 — Auth Service"),
        ("NASIKO_PORT_ROUTER",           "8081 — Query Router"),
        ("NASIKO_PORT_CHAT",             "8083 — Chat History"),
        ("NASIKO_PORT_WEB",              "4000 — Web UI"),
        ("NASIKO_PORT_KONGA",            "1337 — Konga"),
        ("NASIKO_PORT_SERVICE_REGISTRY", "8080 — Agent Discovery"),
        ("NASIKO_PORT_MONGODB",          "27017 — MongoDB"),
        ("NASIKO_PORT_REDIS",            "6379 — Redis"),
        ("NASIKO_PORT_LANGTRACE",        "3000 — Langtrace UI"),
        ("NASIKO_PORT_OTEL_GRPC",        "4317 — OpenTelemetry gRPC"),
        ("NASIKO_PORT_OTEL_HTTP",        "4318 — OpenTelemetry HTTP"),
    ], ("Variable", "Default — Service"))


# ─────────────────────────────────────────────────────────
# DISPATCH
# ─────────────────────────────────────────────────────────

TOPICS = {
    "install":       show_install,
    "quickstart":    show_quickstart,
    "concepts":      show_concepts,
    "auth":          show_auth,
    "cluster":       show_cluster,
    "github":        show_github,
    "agent":         show_agent,
    "chat":          show_chat,
    "observability": show_observability,
    "access":        show_access,
    "local":         show_local,
    "n8n":           show_n8n,
    "images":        show_images,
    "user":          show_user,
    "search":        show_search,
    "env":           show_env,
}


def show_docs(topic: str = None):
    if not topic:
        show_overview()
        return

    fn = TOPICS.get(topic.lower())
    if fn:
        fn()
    else:
        console.print(f"[red]Unknown topic:[/red] '{topic}'\n")
        console.print("Available topics: " + ", ".join(f"[cyan]{t}[/cyan]" for t in TOPICS))
        console.print("\nRun [cyan]nasiko docs[/cyan] for an overview.")
