# Kuma

[![ci](https://github.com/Letdown2491/kuma-linux/actions/workflows/ci.yml/badge.svg)](https://github.com/Letdown2491/kuma-linux/actions/workflows/ci.yml)

**Your system is one file.**

Kuma is a declarative layer over [Fedora bootc](https://docs.fedoraproject.org/en-US/bootc/).
You describe a machine in a small `kuma.toml`, and Kuma compiles it into a
bootable container image. Atomic updates and rollback come from bootc, and
the packages come from Fedora.

![Kuma running the niri desktop: a scrollable tiling Wayland session with waybar, kitty, and fastfetch reporting a read-only overlay root](docs/screenshots/niri.jpg)

## Your declaration

```toml
schema_version = 1

[system]
desktop = "niri"   # curated sets: "niri" or "cosmic"; omit for headless
# base = "quay.io/fedora/fedora-bootc:44"   # optional; see below

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
[`examples/`](examples/niri.toml), and a test keeps every example valid
against the current schema.

**There is no base to name by default.** With `system.base` unset, kuma
composes its own from Fedora's package repos, starting from Fedora's
minimal bootc manifest: bootc, systemd, the kernel, dnf, and the hardware
enablement a real machine needs. Fedora stays the package source; kuma
builds no packages and no kernels. Name a `base` and kuma builds on that
image instead, as any bootc image can be.

`[system].firmware` trims the composed base to your hardware. Unset, it
ships every vendor's, so a machine that declares nothing about its own
still boots with working GPU, wifi, and audio. LVFS firmware updates
refresh on a timer; applying them stays a deliberate `fwupdmgr update`.

Machine state stays out of the file by default. Timezone and hostname
belong to the machine (`timedatectl`, `hostnamectl`) and survive image
updates; pin them here only when you want every machine built from this
file to match.

## How this differs

**NixOS and Guix** own the idea: one versioned file, convergence as the
only way to change anything, rollback for free. Getting there cost them
an entire package universe. Kuma keeps Fedora as the package source and
builds no packages and no kernels, so the declarative property arrives
without an ecosystem to rebuild. Nix's purity guarantees are what you
give up for that.

**Universal Blue** (Bluefin, Bazzite) ships the same three layers:
immutable base, flatpaks for apps, Homebrew for CLI tools. The unit of
configuration is which image you chose. Brewfiles now declare flatpaks
and formulae together, but `brew bundle` is a command you run rather
than a loop that runs without you, and its cleanup decides what to
remove from what is installed rather than from what it installed. Kuma
converges at boot and on a daily timer, and records what it installed,
so an app you added yourself stays yours and `kuma capture` offers to
write it down.

**BlueBuild** builds an image from a recipe and stops at the image. The
recipe never reaches the running machine. Kuma's keeps working after
install: `sync` converges, `diff` reports drift across file, image, and
machine, `kuma.lock` records what the last build resolved to, and
`capture` turns a change you made by hand into a proposal against the
declaration instead of an error to erase.

## Principles

In order:

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

## Status

Kuma is early. It builds, boots, and updates real hardware, and it has not
been run widely. Schema version 1 is meant to be permanent, so the fields
above are promises; everything around them can still move, and
[`CHANGELOG.md`](CHANGELOG.md) is where it says what moved. `kuma switch`
reboots you into a system image, and `bootc` will roll a bad one back, but
try a declaration in `kuma vm` before a machine you depend on.

## Install

Kuma is one self-contained binary: the wallpaper, the greeter config, and
every desktop asset are compiled into it. The published build is static, so
it needs nothing installed alongside it. This matters on the machines most
likely to want kuma, which tend to have podman and no compiler.

This is for the machine you build from. A machine running a kuma image
already has kuma at `/usr/bin/kuma`, baked in by the build that made the
image, and a copy in `/usr/local/bin` would shadow it.

```console
$ curl -LO https://github.com/Letdown2491/kuma-linux/releases/latest/download/kuma-x86_64-unknown-linux-musl
$ chmod +x kuma-x86_64-unknown-linux-musl
$ sudo mv kuma-x86_64-unknown-linux-musl /usr/local/bin/kuma
```

Every release is signed. Each one carries the `cosign verify-blob` command
that checks it came from this repository's release workflow.
[`SECURITY.md`](SECURITY.md) has that command, what a declaration trusts, and
where to report a vulnerability.

Building `main` yourself needs a Rust toolchain at 1.85 or newer and a
linker:

```console
$ cargo install --git https://github.com/Letdown2491/kuma-linux --locked
```

Keep `--locked`. Without it cargo ignores the committed `Cargo.lock` and
resolves dependencies fresh, so you get versions nobody tested.

Cloning also gets you the example declarations and the smoke tests:

```console
$ git clone https://github.com/Letdown2491/kuma-linux
$ cd kuma-linux && cargo install --path .
```

`kuma --version` reports the commit it was built from, and says `-dirty` if
that tree had uncommitted changes. Worth checking when a change you just
made does not show up in the image.

**What needs what.** `init`, `check`, `generate`, and `build` need only
podman. `switch`, `update`, `rollback`, and `doctor` need to be running on a
bootc machine, and a kuma one already has kuma. `vm` and `iso` need KVM and
sudo, except `iso --live`, which needs neither. `install` needs sudo and a
disk you are willing to lose: it is the one command here that cannot be
undone.

## Quick start

```console
$ kuma init          # starter kuma.toml (on a kuma machine: its own declaration)
$ vim kuma.toml      # declare your packages and services
$ kuma build         # podman-builds localhost/kuma:latest
$ kuma switch --yes  # bootc switch; takes effect on next boot
```

Without `--yes`, `switch` only prints what it would do.

## Day 2

The file stays the interface. These read it or edit it for you:

```console
$ kuma                                     # where this machine is, and its next moves
$ kuma add --flatpak org.mozilla.firefox   # declare (--rpm / --brew too)
$ kuma remove org.mozilla.firefox          # drop from whichever list declares it
$ kuma capture                             # declare what this machine already runs
$ kuma check                               # validate the declaration, build nothing
$ kuma diff                                # drift: kuma.toml vs image vs machine
$ kuma doctor                              # machine health, image age, /etc drift, snapshots, GPU
$ kuma sync                                # converge, and update everything installed
$ kuma snapshot                            # the btrfs snapshots this machine has taken
$ kuma snapshot --restore ~/notes.md       # bring a path back (dry run; --yes writes)
$ kuma update --check                      # what has moved in the repos, security first
$ kuma update --yes                        # rebuild on the latest base, stage it
$ kuma rollback --yes                      # boot order back to the previous deployment
$ kuma clean                               # reclaim dangling images, stale bases, build leftovers
```

`add`, `remove`, and `capture` preserve your comments and formatting.
`check`, `diff`, `doctor`, and `update --check` change nothing. Everything
speaks `--json`.

Three more exist for when you need them and never otherwise: `kuma passwd`
hashes a password for `[user]`, `kuma schema` prints the JSON Schema for
`kuma.toml`, and `kuma completions fish | source` wires up your shell.

## Putting it on a machine

```console
$ kuma vm                 # boot the image in a disposable QEMU VM
$ kuma iso --live         # bootable media: the image is its own live session
$ kuma install            # write an image to a disk (destructive)
```

`iso --live` builds media that boots to a working desktop before anything
is written to a disk, because the ISO's root filesystem *is* the image
rather than an installer beside it. `kuma iso` without `--live` builds
Anaconda media instead, which is about a gigabyte larger for the same
system.

`install` pulls `ghcr.io/letdown2491/kuma:niri` unless `--image` names
another, so a machine can be installed from media without building
anything first. It asks which disk, then for an account and a hostname,
because a shared image cannot declare either: the image is shared and you
are not. It
writes them down and the machine creates them on its first boot, the same
way a declared `[user]` works. It partitions the disk itself, so the plan
it prints is the layout it will write: an ESP, a `/boot` outside the root
so encryption stays a later decision rather than a reinstall, and a btrfs
root. It is the one verb here that cannot be undone, so it dry-runs by
default and refuses a disk with anything mounted on it.

## Going deeper

The verbs above are the whole interface. These explain the parts that are
not obvious from them:

- [How kuma behaves](docs/concepts.md): why drift is a proposal rather
  than an error, what `kuma.lock` pins and what it only records, how
  `/etc` is merged rather than replaced, and how a bad update rolls itself
  back.
- [What a desktop contains](docs/desktops.md): what `desktop = "niri"` or
  `"cosmic"` installs that you didn't name, why the surprising parts are
  there, and what you can change.
- [For agents](docs/agents.md): the JSON surface, and why every response
  ends at the legal next commands.
- [Contributing](CONTRIBUTING.md): smoke tests, booting a VM, iterating
  without losing state, what CI checks, and how a release is cut.

## Not yet

- **No encryption at install.** The layout `kuma install` writes puts
  `/boot` outside the root so a passphrase can be added later without a
  different disk shape, but the installer does not offer one yet, so an
  encrypted machine still has to be installed by something else.
- **No offsite backup.** `[snapshots]` survives a mistake, not a dead
  disk. Blocked on where a repository credential lives, since it cannot be
  the declaration.
- **No hibernate.** A swapfile's size and `resume_offset` are properties
  of the installed disk, so it needs a first-boot unit rather than an
  image that already knows the answer.
- **No flatpak permission overrides.** They survive image updates and are
  the one part of the app layer a declaration cannot see or restore.
