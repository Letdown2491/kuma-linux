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

## Quick start

```console
$ kuma init          # writes a starter kuma.toml
$ vim kuma.toml      # declare your packages and services
$ kuma generate      # (optional) inspect the compiled Containerfile
$ kuma build         # podman-builds localhost/kuma:latest
$ kuma switch --yes  # bootc switch; takes effect on next boot
```

`kuma switch` without `--yes` only prints what it would do.

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

[services]
enable = ["tailscaled.service"]
```

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
  bootc-image-builder and boots it in QEMU (login `kuma`/`kuma`, or
  `ssh -p 2222 kuma@localhost` — your ssh key is injected automatically).
  It needs sudo: bootc-image-builder runs as root.

## Roadmap

- [x] `kuma.toml` v1 schema: base, rpm packages, services
- [x] Containerfile generation + local podman build
- [x] `kuma switch` via containers-storage transport
- [x] `kuma vm` — qcow2 via bootc-image-builder, booted in QEMU
- [ ] `kuma diff` — show what an apply would change
- [ ] `kuma update` — pull newer base, rebuild, apply
- [x] Flatpaks: Flathub remote in-image, declared apps converged on boot
- [x] Declarative user: created on first boot, converged after (/home is machine state)
- [ ] `kuma sync` — on-demand runtime state sync (flatpaks without reboot, later user config)
- [ ] Base images (`kuma-gnome`, `kuma-plasma`) built in CI
- [ ] Installer ISO via bootc-image-builder
