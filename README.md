# Kuma

[![ci](https://github.com/Letdown2491/kuma-linux/actions/workflows/ci.yml/badge.svg)](https://github.com/Letdown2491/kuma-linux/actions/workflows/ci.yml)

**Your system is one file.**

Kuma is a declarative layer over [Fedora bootc](https://docs.fedoraproject.org/en-US/bootc/).
You describe a machine in a small, readable `kuma.toml`, and Kuma compiles it
into a bootable container image. Atomic updates and rollback come from bootc,
the packages come from Fedora, and Kuma is the experience on top.

Four principles, in order:

1. **Simple.** The schema stays small and boring. Every field is a promise
   kept forever, so new ones have to earn their place.
2. **Atomic.** Applying a declaration never mutates the running system: it
   builds an image and switches to it on next boot. Rollback is always
   available, and automatic when an update can't boot to a healthy system.
3. **Local-first.** `kuma build` needs nothing but podman. No forge account,
   no CI, no registry. All optional, later.
4. **Self-describing.** Every command reports where you are and ends at the
   legal next commands, never a dead end. The image carries the declaration
   it was built from, so a machine can always speak for itself.

**Status.** Kuma is early. It builds, boots, and updates real hardware, and
it has not been run widely. Schema version 1 is meant to be permanent, so
the fields below are promises; everything around them can still move. `kuma switch`
reboots you into a system image, and `bootc` will roll a bad one back, but
try a declaration in `kuma vm` before you try it on a machine you need
tomorrow.

## Install

Kuma is one self-contained binary: the wallpaper, the greeter config, and
every desktop asset are compiled into it, so it needs nothing beside it on
disk. Building it needs a Rust toolchain at 1.85 or newer and a linker.

```console
$ cargo install --git https://github.com/Letdown2491/kuma-linux --locked
```

Or clone it, which also gets you the example declarations and the smoke
tests:

```console
$ git clone https://github.com/Letdown2491/kuma-linux
$ cd kuma-linux && cargo install --path .
```

Either way the binary is `kuma`.

Keep `--locked` on the first form. Without it cargo ignores the committed
`Cargo.lock` and resolves dependencies fresh, so you get versions nobody
tested.

**What needs what.** `init`, `check`, `generate`, and `build` need only
podman. `switch`, `update`, `rollback`, and `doctor` need to be running on a
bootc machine. `vm` and `iso` need KVM and sudo.

One catch worth knowing before you start: if you already run an image-based
desktop, which is the obvious place to want this, you probably have podman
and no compiler. Build kuma in a toolbox or a container and copy the binary
out. Prebuilt binaries are on the roadmap for exactly this reason.

## Quick start

```console
$ kuma init          # starter kuma.toml (on a kuma machine: its own declaration)
$ vim kuma.toml      # declare your packages and services
$ kuma build         # podman-builds localhost/kuma:latest
$ kuma switch --yes  # bootc switch; takes effect on next boot
```

Without `--yes`, `switch` only prints what it would do.

## Your declaration

```toml
schema_version = 1

[system]
base = "quay.io/fedora/fedora-bootc:44"
desktop = "niri"   # curated sets: "niri" or "cosmic"; omit for headless

[user]
name = "me"
shell = "fish"
# password_hash = '$6$...'   # from `kuma passwd`; applies only at creation

[packages]
rpm = ["fish", "distrobox", "tailscale"]
flatpak = ["org.mozilla.firefox"]   # from Flathub, converged on boot
brew = ["ripgrep", "gh"]            # CLI tools, no rebuild needed

[services]
enable = ["tailscaled.service"]

[snapshots]
enable = true   # hourly read-only btrfs snapshots of /var/home
```

A fuller, commented version lives in
[`examples/`](examples/kuma.toml.example), and a test keeps every example
valid against the current schema.

Machine state stays out of the file by default. Timezone and hostname
belong to the machine (`timedatectl`, `hostnamectl`) and survive image
updates; pin them here only when you want every machine built from this
file to match.

## Day 2

The file stays the interface. These read it or edit it for you:

```console
$ kuma                                     # where this machine is, and its next moves
$ kuma add --flatpak org.mozilla.firefox   # declare (--rpm / --brew too)
$ kuma remove org.mozilla.firefox          # drop from whichever list declares it
$ kuma capture                             # declare what this machine already runs
$ kuma check                               # validate the declaration, build nothing
$ kuma diff                                # drift: kuma.toml vs image vs machine
$ kuma doctor                              # machine health, /etc drift, snapshots, GPU, disk
$ kuma sync                                # converge flatpaks and brew now
$ kuma snapshot                            # the btrfs snapshots this machine has taken
$ kuma snapshot --restore ~/notes.md       # bring a path back (dry run; --yes writes)
$ kuma update --check                      # has the locked base moved?
$ kuma update --yes                        # rebuild on the latest base, stage it
$ kuma rollback --yes                      # boot order back to the previous deployment
$ kuma clean                               # reclaim dangling images, stale bases, build leftovers
```

`add`, `remove`, and `capture` preserve your comments and formatting.
`check`, `diff`, `doctor`, and `update --check` change nothing. Everything
speaks `--json`.

## Going deeper

The verbs above are the whole interface. These explain the parts that are
not obvious from them:

- [How kuma behaves](docs/concepts.md): why drift is a proposal rather
  than an error, what `kuma.lock` pins and what it only records, how
  `/etc` is merged rather than replaced, and how a bad update rolls itself
  back.
- [For agents](docs/agents.md): the JSON surface, and why every response
  ends at the legal next commands.
- [Contributing](CONTRIBUTING.md): smoke tests, booting a VM, iterating
  without losing state, and what CI checks.

## Roadmap

Shipped: the v1 schema and image build, `switch`, `vm`, `iso`, the day-2
verbs, two curated desktops (niri and COSMIC), declarative users, flatpak
and brew convergence that takes back only what it installed, declarative
btrfs snapshots with `kuma snapshot` to reach them, firmware updates via
fwupd, boot health with automatic rollback, `kuma.lock`, `/etc` drift
detection, build-and-boot smoke tests, and a JSON surface for agents.

Next:

- [ ] Registry publishing and CI builds, so an image is `bootc switch`-able
      from anywhere, signed.
- [ ] Offsite backup to complement `[snapshots]`, which only survives a
      mistake and not a dead disk. Blocked on where a repo credential
      lives, since it cannot be the declaration.
- [ ] Hibernate. A swapfile's size and `resume_offset` are properties of
      the installed disk, so it needs a first-boot unit rather than an
      image that already knows the answer.
- [ ] Flatpak permission overrides, which survive image updates and are
      the one part of the app layer the declaration cannot see.
