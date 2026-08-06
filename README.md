# Kuma

**Your system is one file.**

Kuma is a declarative layer over [Fedora bootc](https://docs.fedoraproject.org/en-US/bootc/):
you describe your system in a small, readable `kuma.toml`, and Kuma compiles it
into a bootable container image. Atomic updates and rollback come from bootc;
the packages come from Fedora; the only thing Kuma adds is the experience.

Design principles, in order:

1. **Simple** — the config schema stays small and boring. Every field is a
   promise maintained forever, so new fields have to earn their place.
2. **Atomic** — applying a config never mutates the running system; it builds a
   new image and switches to it on next boot. Rollback is always available.
3. **Local-first** — `kuma build` works with nothing but podman. No forge
   account, no CI pipeline, no registry required (all optional later).
4. **Self-describing** — hypermedia as the engine of system state: every
   output names where you are and ends at the legal next commands, never a
   dead end. Bare `kuma` is the root resource — it reports the machine's
   state (edited, staged, drifted, in sync, ...) and its next moves,
   computed from the machine rather than hand-written; `kuma --json`
   serves the same map to scripts and agents. Doctor findings carry their
   fix the same way. The image carries the kuma.toml it was built from
   (`/usr/lib/kuma/kuma.toml`), so a machine can always speak for itself —
   and `kuma init` on a kuma machine seeds from that copy, not a template.

## Quick start

```console
$ kuma init          # writes a starter kuma.toml
$ vim kuma.toml      # declare your packages and services
$ kuma generate      # (optional) inspect the compiled Containerfile
$ kuma build         # podman-builds localhost/kuma:latest
$ kuma switch --yes  # bootc switch; takes effect on next boot
```

`kuma switch` without `--yes` only prints what it would do.

Day 2, the file stays the interface — these edit or read it for you:

```console
$ kuma                                     # bare: current state + next actions (--json for agents)
$ kuma add --flatpak org.mozilla.firefox   # declare in kuma.toml (--rpm/--brew too)
$ kuma remove org.mozilla.firefox          # drop from whichever list declares it
$ kuma diff                                # drift: kuma.toml vs image vs machine
$ kuma doctor                              # machine health: deployment, convergence, GPU, storage, disk
$ kuma sync                                # converge flatpaks/brew now, not at next boot
$ kuma update --yes                        # pull newer base, rebuild, stage for next boot
$ kuma clean                               # reclaim dangling images and abandoned build containers
```

`add` and `remove` preserve your comments and formatting; `diff` and
`doctor` are read-only.

Without `--config`, kuma uses `./kuma.toml`, falling back to
`~/.config/kuma/kuma.toml` — a home for declarations that don't live in a
project checkout. Neither is ever created implicitly; `kuma init` is how a
declaration comes to exist, and on a kuma machine it writes a copy of the
machine's own baked declaration (`--starter` for the generic template).

## Example config

```toml
schema_version = 1

[system]
base = "quay.io/fedora/fedora-bootc:44"
desktop = "niri"   # curated desktop set; omit for a headless system

[user]
name = "me"
shell = "fish"
password_hash = '...'   # from `kuma passwd`; applies only at creation

[packages]
rpm = ["fish", "distrobox", "tailscale"]
flatpak = ["org.mozilla.firefox"]   # from Flathub, synced on boot
brew = ["ripgrep", "gh"]            # CLI tools, no rebuild needed

[services]
enable = ["tailscaled.service"]
```

A fuller, commented example lives at
[`examples/kuma.toml.example`](examples/kuma.toml.example); a test keeps it
valid against the current schema.

## Developing and testing

- **CLI development** works anywhere Rust does, including a distrobox. When
  kuma runs inside a container it drives the *host's* podman/bootc via
  `flatpak-spawn --host`.
- **Inspecting a built image**: it's a normal OCI image, so
  `podman run --rm -it localhost/kuma:latest bash` or even
  `distrobox create --image localhost/kuma:latest` work fine for poking at
  the userland.
- **Boot testing** (the real thing — systemd, kernel args, `bootc switch`,
  rollback) can't happen in a container. `kuma vm` builds a qcow2 via
  bootc-image-builder and boots it in QEMU. Log in as your declared
  `[user]` (created on first boot), or the always-present test user
  `kuma`/`kuma` (`ssh -p 2222 kuma@localhost` — your ssh key is injected
  automatically). It needs sudo: bootc-image-builder runs as root.
  After a rebuild of the image, pass `--rebuild` — kuma warns when the
  reused disk is older than the image.
- **Iterating without losing state**: `kuma vm --apply` streams the freshly
  built image into the *running* VM and `bootc switch`es inside it. The VM
  reboots into the new image with `/var` intact — flatpaks, brew, and homes
  survive, so nothing re-downloads. It's also the real update path (staged
  deployment; `bootc rollback` inside the VM undoes it). Use `--rebuild`
  only when you want a factory-fresh machine.
- **Installer media**: `kuma iso` builds an Anaconda installer ISO from the
  image (`iso/bootiso/install.iso`) — bootable in GNOME Boxes or `dd`'d to a
  USB stick. The install is interactive (language, disk), but kuma-owned
  choices are preseeded: hostname, no initial-setup, and — when kuma.toml
  declares a `[user]` — no Anaconda user screen, since the declared account
  is created on first boot. Unlike `kuma vm` disks, ISOs carry no test user.

## Roadmap

- [x] `kuma.toml` v1 schema: base, rpm packages, services
- [x] Containerfile generation + local podman build
- [x] `kuma switch` via containers-storage transport
- [x] `kuma vm` — qcow2 via bootc-image-builder, booted in QEMU
- [x] `kuma iso` — Anaconda installer ISO for real hardware and Boxes
- [x] `kuma diff` — drift between the declaration, the image, and the machine
- [x] `kuma add` / `kuma remove` — edit the declaration in place, comments intact
- [x] `kuma doctor` — deployment, convergence, GPU, and disk health checks
- [x] `kuma update` — pull the newer base, rebuild, stage in one step
- [x] Flatpaks: Flathub remote in-image, declared apps converged on boot
- [x] Declarative user: created on first boot, converged after (/home is machine state)
- [x] `kuma passwd` — hash a password for the `[user]` section
- [x] `kuma sync` — converge flatpaks and brew on demand (user config later)
- [x] Bare `kuma` — the state machine as hypermedia: state + next actions, human and JSON
- [ ] Registry publishing + CI builds (`bootc switch`-able from anywhere)
- [ ] `kuma.lock` — pin base digest and package versions; `kuma update` moves pins deliberately
