# agents

## Building and testing

This host has no C compiler and no glibc development files: `cargo build`
and `cargo test` cannot link here, not even for build scripts. The build
environment is the `kuma-dev-gcc` container, which carries gcc and shares
the toolchain:

```
podman run --rm -v /var/home/martin/Documents/kuma:/kuma:Z \
  -v /var/home/martin/.cargo:/mnt/cargo:Z -v /var/home/martin/.rustup:/mnt/rustup:Z \
  -w /kuma -e RUSTUP_HOME=/mnt/rustup -e CARGO_HOME=/mnt/cargo \
  localhost/kuma-dev-gcc bash -c 'export PATH=/mnt/cargo/bin:$PATH; cargo test'
```

Host `cargo check` is a trap, not a shortcut: it works until the
container rebuilds `target/`, then poisons the shared artifacts in both
directions. Build and test in the container only.

## Commit before build

Commit everything first, then build. The build script stamps the working
tree's identity into the binary (`build.rs`): an uncommitted tree ships a
`-dirty` binary, and a binary installed locally with that stamp is one
that answers for code no commit describes. The tree that produced a
running binary should always be a commit you can name.

## Agent skills

### Issue tracker

Issues are tracked as GitHub Issues on Letdown2491/kumaOS via the `gh` CLI. See `docs/agents/issue-tracker.md`.

### Triage labels

The five canonical triage roles, each label string equal to its name. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: `CONTEXT.md` + `docs/adr/` at the repo root. See `docs/agents/domain.md`.
