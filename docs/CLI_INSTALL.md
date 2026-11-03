# Installing the Nasiko CLI

One command, no account and no administrator rights.

macOS and Linux:

```console
$ curl -fsSL https://registry.nasiko.dev/r/nasiko/install | bash
```

Windows (PowerShell):

```powershell
irm https://registry.nasiko.dev/r/nasiko/install-ps1 | iex
```

Then check it: `nasiko --version`.

## What the installer does

The CLI is published to the Nasiko registry as one multi-platform artifact, `nasiko/nasiko`, with a build for
macOS (Apple silicon and Intel), Linux (x86-64 and arm64, statically linked) and Windows (x86-64). The installer
([`scripts/install.sh`](../scripts/install.sh), [`scripts/install.ps1`](../scripts/install.ps1)):

1. Detects the operating system and CPU.
2. Fetches the release's OCI image index and picks the build for this platform.
3. Downloads that build and **verifies its SHA-256 against the registry's content-addressed digest** before
   installing anything.
4. Installs it for the current user only:
   - macOS and Linux: `~/.local/bin/nasiko`. It never uses `sudo` and never edits your shell profile. If that
     folder isn't on your `PATH`, or another `nasiko` comes before it, it says so and prints the line to add.
   - Windows: `%LOCALAPPDATA%\Programs\nasiko\nasiko.exe`, added to your *user* `PATH` (open a new terminal).

The macOS/Linux script needs `bash`, `curl`, `sha256sum` or `shasum`, and `jq` or `python3`. If piping a script
into a shell isn't allowed where you are, download it, read it, then run it: it's short.

## Options

| Variable | Default | Purpose |
|---|---|---|
| `NASIKO_VERSION` | `latest` | Install a specific release, e.g. `v0.2.0` |
| `NASIKO_INSTALL_DIR` | `~/.local/bin` (Windows: `%LOCALAPPDATA%\Programs\nasiko`) | Where to install |
| `NASIKO_REGISTRY` | `registry.nasiko.dev` | Registry to install from (a mirror, say) |

```console
$ curl -fsSL https://registry.nasiko.dev/r/nasiko/install | NASIKO_VERSION=v0.2.0 bash
```

## Updating and removing

Run the installer again to update. To remove the CLI, delete the binary (`rm ~/.local/bin/nasiko`, or the
`nasiko` folder under `%LOCALAPPDATA%\Programs` on Windows) and, if you no longer want its settings,
`~/.nasiko/`.

## Building from source

`cargo install --path cli` from a checkout installs the same CLI from source.
