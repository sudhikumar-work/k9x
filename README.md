# k9x

[![CI](https://github.com/sudhikumar-work/k9x/actions/workflows/ci.yml/badge.svg)](https://github.com/sudhikumar-work/k9x/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/sudhikumar-work/k9x?color=blue&label=release)](https://github.com/sudhikumar-work/k9x/releases)
[![Docker Image](https://img.shields.io/badge/docker-ghcr.io%2Fsudhikumar--work%2Fk9x-blue?logo=docker)](https://github.com/sudhikumar-work/k9x/pkgs/container/k9x)
[![License](https://img.shields.io/badge/license-Apache%202.0-blue.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Linux%20%7C%20WSL%20%7C%20Windows-lightgrey.svg)](https://github.com/sudhikumar-work/k9x/releases)

**k9x** is an ultra-fast, event-driven Kubernetes TUI + headless agent CLI — built in Rust for humans *and* AI agents at equal speed.

---

## Why k9x?

| Metric / Feature | k9s | **k9x** | Advantage |
|---|---|---|---|
| **Refresh Architecture** | Timer polling (2s re-list + full re-render) | **Watch streams → render only on delta** | Zero idle API churn |
| **Startup Latency** | Blocks on informer `WaitForCacheSync` | **First frame in < 1ms**, rows stream in | Instant launch (<60ms TTFD) |
| **Process Exec (CLI p50)** | ~48.1ms | **5.4ms** | ~8.9x faster |
| **Binary Footprint** | ~133.9MB | **~7.3MB** (LTO + Strip) | ~18x smaller |
| **Idle Memory (RSS)** | ~88.0MB | **~11.4MB** | ~7.7x less RAM |
| **Agent / Headless Mode** | TUI only | **Native CLI**: `ls/get/logs/watch` with `-o json/yaml/name` | AI Agent & Script Ready |

---

## Installation

### 1. One-Line Install Script (macOS, Linux & WSL)

Automatically detects your operating system and architecture, verifies SHA-256 integrity, and installs the standalone binary:

```bash
curl -fsSL https://raw.githubusercontent.com/sudhikumar-work/k9x/main/install.sh | bash
```

Alternatively using `wget`:
```bash
wget -qO- https://raw.githubusercontent.com/sudhikumar-work/k9x/main/install.sh | bash
```

To install a specific version or customize the destination directory:
```bash
curl -fsSL https://raw.githubusercontent.com/sudhikumar-work/k9x/main/install.sh | bash -s -- --version v0.2.5 --dir ~/.local/bin
```

---

### 2. Homebrew (macOS, Linux & WSL)

Install via the official Homebrew tap:

```bash
brew install sudhikumar-work/tap/k9x
```

Or by tapping first:
```bash
brew tap sudhikumar-work/tap
brew install k9x
```

To update:
```bash
brew update && brew upgrade k9x
```

---

### 3. Docker / Container Image

Run `k9x` instantly in an isolated, minimal container mounted with your local kubeconfig:

```bash
docker run -it --rm \
  -v ~/.kube/config:/root/.kube/config:ro \
  ghcr.io/sudhikumar-work/k9x:latest
```

---

### 4. Cargo (Rust toolchain)

If you have Rust and `cargo` installed:

```bash
cargo install --git https://github.com/sudhikumar-work/k9x
```

---

### 5. Direct Binary Download

Pre-compiled, standalone binaries are available on the [GitHub Releases Page](https://github.com/sudhikumar-work/k9x/releases):

| Operating System | Architecture | Package Archive |
|---|---|---|
| **macOS** | Apple Silicon (`arm64`) | `k9x-<version>-darwin-arm64.tar.gz` |
| **macOS** | Intel (`x86_64`) | `k9x-<version>-darwin-amd64.tar.gz` |
| **macOS** | Universal Binary | `k9x-<version>-darwin-universal.tar.gz` |
| **Linux / WSL** | `x86_64` (amd64) | `k9x-<version>-linux-amd64.tar.gz` |
| **Linux / WSL** | `aarch64` (arm64) | `k9x-<version>-linux-arm64.tar.gz` |
| **Windows** | `x86_64` (amd64) | `k9x-<version>-windows-amd64.zip` |

Verify downloads using the published `checksums.txt`:
```bash
sha256sum -c checksums.txt
```

> **macOS Gatekeeper Note:** If running a manually downloaded binary triggers a Gatekeeper warning, clear the quarantine attribute:
> ```bash
> xattr -d com.apple.quarantine /path/to/k9x
> ```

---

## Usage

### 1. Human Mode (Interactive TUI)

```bash
k9x                    # Launch TUI in current kubeconfig context & namespace
k9x deploy             # Jump directly to Deployments view
k9x -A                 # Start in all-namespaces mode
k9x -r                 # READ-ONLY mode: mutation actions strictly blocked
```

#### Keybindings

| Key | Action |
|---|---|
| `Enter` | View child items / actions |
| `l` | Live log stream (press `/` to search with real-time hit counts) |
| `s` | Open interactive shell into container |
| `d` | Run `kubectl describe` in-app |
| `y` | Inspect resource YAML |
| `e` | Live edit resource (`$EDITOR` with Server-Side Apply) |
| `p` | Manage Port Forwarding |
| `X` | Decode Secret values |
| `R` | Restart Deployment / DaemonSet / StatefulSet |
| `S` | Scale Replicas |
| `t` | Trigger CronJob execution |
| `x` | Toggle Job / CronJob suspension |
| `c` / `u` / `D` | Node actions: Cordon / Uncordon / Drain |
| `Ctrl+D` / `Ctrl+K` | Delete / Force delete resource (requires confirmation) |
| `/` | Filter rows in real-time |
| `Tab` | Cycle column sorting |
| `a` | Toggle All-Namespaces |
| `:` | Command palette (`ctx`, `ns`, `pulse`, `pf`, `crds`, `helm`, `xray`, `popeye`, `q`) |
| `?` | Help overlay |

#### Built-in Diagnostics & EKS Lifecycle Windows
- **EKS Support Windows:** Header displays standard (`std`) and extended (`ext`) support end dates live from AWS EKS APIs (cached in `~/.config/k9x/eks-support.json`).
- **Cluster Resource Metering:** Real-time CPU and Memory utilization with percentage gauges.
- **Health Tinting:** Immediate visual feedback (red for crashing/evicted pods, orange for degraded or restarting workloads).

---

### 2. Agent Mode (Fast Headless CLI)

Sub-6ms invocation speed and structured outputs designed for automation scripts and AI agents:

```bash
# Structured JSON query
k9x ls po -A -o json | jq '.[].metadata.name'

# Real-time event watch stream (JSON Lines format)
k9x ls po --watch

# Fast log inspection
k9x logs mypod -c sidecar -f --tail 100

# Fetch YAML spec
k9x get deploy web -o yaml

# Inspect Nodes and Contexts
k9x describe node ip-10-0-0-1
k9x ctx && k9x ns

# Safe mutations (CLI mutations require explicit --yes)
k9x scale deploy web 3 --yes
k9x del po badpod --yes
```

---

## Configuration

Custom configuration is loaded from `${XDG_CONFIG_HOME:-~/.config}/k9x/config.toml` (override with `$K9X_CONFIG`):

```toml
context         = ""      # empty = use active kubeconfig context
namespace       = ""      # empty = use active kubeconfig namespace
all_namespaces  = false
readonly        = false
default_view    = "po"
tick_ms         = 200     # UI render heartbeat in milliseconds
log_tail        = 5000
log_cap         = 50000   # in-memory log buffer capacity
theme           = "dark"  # "dark" | "light" | "mono"
```

### Custom Columns (`views.yml`)

Customize table columns per resource in `~/.config/k9x/views.yml` — reorder columns,
add custom JSON-path columns, and adjust relative widths:

```yaml
views:
  po:                          # match by alias, plural, or kind (po / pods / Pod)
    order: [NODE, NAME, STATUS, AGE]   # reorder columns (case-insensitive;
                                       # unlisted columns keep their relative order)
    widths:                    # relative width weights (1..=100)
      NAME: 5
      age: 1
    append_columns:            # add custom columns extracted via JSON path
      - name: NODE
        path: spec.nodeName
        weight: 4              # optional width weight for the custom column
      - name: IP
        path: status.podIP
  deploy:
    replace_columns: true      # replace ALL built-in columns instead of appending
    columns:
      - name: NAME
        path: metadata.name
```

### Plugins

`k9x` supports plugins configured in `~/.config/k9x/plugins.yml` (override with `$K9X_PLUGINS`):

```yaml
plugin:
  tail:
    shortCut: ctrl-t
    description: follow logs
    scopes: [po]
    command: kubectl
    background: false
    args: ["logs", "-f", "$NAME", "-n", "$NAMESPACE"]
```

---

## Safety & Governance

- **Zero Unintended Mutations:** The `-r` / `--readonly` flag guarantees that all destructive actions, edits, and deletions are disabled in both TUI and CLI.
- **Explicit CLI Confirmation:** All mutation CLI commands require the explicit `--yes` flag to execute.
- **Confirmation Prompts:** Destructive TUI actions (delete, drain, restart) require keyboard confirmation.

---

## Inspiration & Giving Back

When I began my journey in DevOps over four and a half years ago, the open-source Kubernetes ecosystem and tools like **k9s** were foundational to my learning and daily work. I have immense gratitude for Fernand Galiana and the contributors whose pioneering work set the gold standard for terminal cluster management.

As production clusters scaled and incident response demanded sub-millisecond triage with zero idle API churn, I built **k9x** out of necessity — architecting an event-driven, delta-stream async engine in Rust. `k9x` is my humble contribution back to the incredible DevOps and Kubernetes community that taught me so much. I hope it helps fellow engineers, operators, and agents navigate their clusters with speed and joy.

---

## Author & Maintainer

Created and maintained with ❤️ by **Sudheeshkumar Surendran** ([@sudhikumar-work](https://github.com/sudhikumar-work)).

---

## License

`k9x` is licensed under the **[Apache License, Version 2.0](LICENSE)**.
