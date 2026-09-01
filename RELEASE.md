# `k9x` Open Source Release & Distribution Plan

This document details the release and distribution strategy for `k9x` as an open-source tool published under the **Apache License 2.0** by **Sudheeshkumar Surendran** ([@sudhikumar-work](https://github.com/sudhikumar-work)).

---

## 1. Repository Topology & Ecosystem

| Repository | Visibility | Role & Contents |
|---|---|---|
| [`sudhikumar-work/k9x`](https://github.com/sudhikumar-work/k9x) | **Public** | **Main Open Source Repository**: Rust source code, tests, CI/CD workflows (`.github/workflows/`), GitHub Releases, and documentation (`README.md`, `LICENSE`, `install.sh`, `Makefile`). |
| [`sudhikumar-work/homebrew-tap`](https://github.com/sudhikumar-work/homebrew-tap) | **Public** | **Homebrew Tap**: Formula `Formula/k9x.rb` allowing users to install via `brew install sudhikumar-work/tap/k9x`. |

---

## 2. Release Automation Workflow (`.github/workflows/release.yml`)

The automated release workflow triggers on any semver git tag push (`v*.*.*`):

1. **Matrix Build Targets**:
   - macOS Apple Silicon (`aarch64-apple-darwin`)
   - macOS Intel (`x86_64-apple-darwin`)
   - macOS Universal Binary (`lipo -create`)
   - Linux x86_64 (`x86_64-unknown-linux-musl` / `gnu`)
   - Linux ARM64 (`aarch64-unknown-linux-musl` / `gnu`)
   - Windows x86_64 (`x86_64-pc-windows-msvc`)
2. **Artifact Packaging & Checksums**:
   - Tarballs (`.tar.gz`) and Zip archives (`.zip`) created with binary, `README.md`, and `LICENSE`.
   - `checksums.txt` generated with `sha256sum`.
3. **GitHub Release Publication**:
   - Assets and release notes published to `sudhikumar-work/k9x/releases`.
4. **Homebrew Formula Update** (hardened tap-publish pipeline):
   - Automated formula update in `sudhikumar-work/homebrew-tap` with new release URLs and SHA-256 hashes.
   - Pre-publish validation gates: all four SHA256 checksums validated as 64-char hex **before** the tap is touched; the rendered formula is rejected if it contains git conflict markers, leftover `REPLACE_WITH_*` placeholder tokens, or fails `ruby -c` syntax checking.
   - Fail-loud push semantics (no `|| true` swallowing), idempotent no-op skip, rebase-before-push on tap divergence, and a workflow-level `concurrency` group preventing release-run races.

---

## 3. Installation Channels

### 1. Universal One-Line Installer (macOS, Linux, WSL)
```bash
curl -fsSL https://raw.githubusercontent.com/sudhikumar-work/k9x/main/install.sh | bash
```
*(or with `wget`)*:
```bash
wget -qO- https://raw.githubusercontent.com/sudhikumar-work/k9x/main/install.sh | bash
```

### 2. Homebrew (macOS, Linux, WSL)
```bash
brew install sudhikumar-work/tap/k9x
```
*(or via tap)*:
```bash
brew tap sudhikumar-work/tap && brew install k9x
```

### 3. Cargo (Rust toolchain)
```bash
cargo install --git https://github.com/sudhikumar-work/k9x
```

---

## 4. Release Checklist

- [x] Adopt Apache License 2.0 with proper copyright attribution.
- [x] Update `Cargo.toml` with author metadata and dependencies.
- [x] Run full test matrix (`cargo test` + `python3 tests/matrix.py`).
- [x] Regenerate shell completions for `bash`, `zsh`, `fish`.
- [x] Push to public GitHub repository `sudhikumar-work/k9x`.
- [x] Tag release `v0.2.4` and trigger CI release workflow.

> For each new release: bump `version` in `Cargo.toml`, tag `vX.Y.Z`, and push the tag —
> the release workflow builds all platforms and updates the Homebrew tap automatically.
