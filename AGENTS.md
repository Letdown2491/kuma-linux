# agents

## Building and testing

This host has no C compiler and no glibc development files: `cargo build`
and `cargo test` cannot link here, not even for build scripts. The build
environment is the `kuma-dev-gcc` container, which carries gcc and shares
the toolchain:

```
# from the checkout's root; ~ is this machine's home, which is the
# only identity the tree is allowed to carry
podman run --rm -v "$PWD":/kuma:Z \
  -v ~/"\.cargo":/mnt/cargo:Z -v ~/"\.rustup":/mnt/rustup:Z \
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

## The nostr layer's dev loop (44.4.0)

The daemon on a kuma machine is deployed from the checkout: a user-level
drop-in (`~/.config/systemd/user/kuma-nostrd.service.d/override.conf`)
re-points `ExecStart` at `target/release/kuma-nostrd`, marked TEMPORARY
until the image ships the fixed daemon. So the deploy is: commit, then
`cargo build --release` + `cargo install --path .` in the container
(the CLI the panel spawns comes from `~/.cargo/bin`; the daemon runs
from `target/release`), then `systemctl --user restart kuma-nostrd`.

The panel is a noctalia path source at
`~/.local/state/noctalia/plugins/kuma-nostr/`, extracted from
`NOSTR_PLUGIN_TREE` in `src/containerfile/blocks.rs` (a python regex
over the `r#"..."#` blocks works; verify by comparing hashes with the
staging manifest), then `noctalia msg plugins update kuma-nostr`. The
host retires entries that error or time out — resurrect with the
settings.toml road: set `[plugins] enabled = []`, then back to
`["kuma/nostr"]`; the config watcher re-loads. `noctalia msg plugins
enable` is broken upstream (parse error on a clean path source) — do
not use it. Verify reloads in the shell's journal
(`journalctl --user -u kuma-shell`): a `luau_load failed` line means a
broken deploy, a `hot reload: reloaded` line a good one.

## The noctalia host API's loaded facts (each learned the hard way)

- A Luau local read before its declaration exists resolves to the
  global — nil. The panel forward-declares `render` for this reason;
  every helper a callback calls must be declared above the callback's
  definer too.
- A flex container (column/row/scroll) centers its children on the
  cross axis by default: pass `align = "stretch"` for full-width. The
  docs page says the stretch is the default; the reference
  implementation's own layout notes say center. The notes are right.
- A clickable container (onClick on row/column) is wrapped
  content-sized by the host: the card's width does not survive it.
  Clicks go on buttons or inner rows.
- A fetch's failure is not a fact about the world: keep last-known
  state, or the empty state impersonates the list for a poll cycle.
- `NoDisplay=true` on a scheme-handler desktop file hides it from
  xdg-desktop-portal's chooser — a flatpak browser then reports "no
  supported apps". Handlers stay visible.
- The argv form of `runAsync` (plugin_api 24) is the road for every
  argument that is not a literal: a nostrconnect URI joined into a
  shell line is shattered by its own `&`s.
- `noctalia plugins lint` checks manifests, not Luau bodies; the
  goldens pin bytes. The balance test (`the_plugin_lua_balances`) is
  the only parse-shape gate — keep it honest.

## Agent skills

### Issue tracker

Issues are tracked as GitHub Issues on Letdown2491/kumaOS via the `gh` CLI. See `docs/agents/issue-tracker.md`.

### Triage labels

The five canonical triage roles, each label string equal to its name. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: `CONTEXT.md` + `docs/adr/` at the repo root. See `docs/agents/domain.md`.
