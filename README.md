# Kuma

**Your system is one file.**

Kuma is a declarative layer over [Fedora bootc](https://docs.fedoraproject.org/en-US/bootc/):
you describe your system in a small, readable `kuma.toml`, and Kuma compiles it
into a bootable container image. Atomic updates and rollback come from bootc;
the packages come from Fedora; the only thing Kuma adds is the experience.

Design principles, in order:

1. **Simple**: the config schema stays small and boring. Every field is a
   promise maintained forever, so new fields have to earn their place.
2. **Atomic**: applying a config never mutates the running system; it builds a
   new image and switches to it on next boot. Rollback is always available,
   and automatic when a fresh update can't boot to a healthy system: every
   image bakes [greenboot](https://github.com/fedora-iot/greenboot-rs), a GRUB
   boot counter falls back to the previous deployment after three failed
   attempts, and desktop images count a boot as healthy only once the greeter
   is actually on screen. A bad update costs reboots, not the machine.
3. **Local-first**: `kuma build` works with nothing but podman. No forge
   account, no CI pipeline, no registry required (all optional later).
4. **Self-describing**: hypermedia as the engine of system state: every
   output names where you are and ends at the legal next commands, never a
   dead end. Bare `kuma` is the root resource: it reports the machine's
   state (edited, staged, drifted, in sync, ...) and its next moves,
   computed from the machine rather than hand-written; `kuma --json`
   serves the same map to scripts and agents, and `doctor --json` /
   `diff --json` extend it to machine health and drift. Doctor findings
   carry their fix the same way. The image carries the kuma.toml it was built from
   (`/usr/lib/kuma/kuma.toml`), so a machine can always speak for itself,
   and `kuma init` on a kuma machine seeds from that copy, not a template.

## Quick start

```console
$ kuma init          # starter kuma.toml (on a kuma machine: the machine's own declaration)
$ vim kuma.toml      # declare your packages and services
$ kuma generate      # (optional) inspect the compiled Containerfile
$ kuma build         # podman-builds localhost/kuma:latest
$ kuma switch --yes  # bootc switch; takes effect on next boot
```

`kuma switch` without `--yes` only prints what it would do.

Day 2, the file stays the interface; these edit or read it for you:

```console
$ kuma                                     # bare: current state + next actions (--json for agents)
$ kuma add --flatpak org.mozilla.firefox   # declare in kuma.toml (--rpm/--brew too)
$ kuma remove org.mozilla.firefox          # drop from whichever list declares it
$ kuma capture                             # declare what this machine already runs (--yes to write)
$ kuma check                               # validate the declaration without building anything
$ kuma diff                                # drift: kuma.toml vs image vs machine
$ kuma doctor                              # machine health: deployment, boot health, convergence, GPU, storage, disk
$ kuma sync                                # converge flatpaks/brew now, not at next boot
$ kuma update --yes                        # pull latest base, rebuild, stage for next boot
$ kuma rollback --yes                      # the update's undo: boot order back to the previous deployment
$ kuma clean                               # reclaim dangling images and abandoned build containers
```

`add`, `remove`, and `capture` preserve your comments and formatting;
`check`, `diff`, and `doctor` are read-only, and everything speaks `--json`
(see below).

## Drift is a fork, not an error

Declarative systems normally treat drift as failure: the machine deviates,
the tool corrects it, the deviation is erased. That is why imperative
escape hatches always feel like cheating, and why the thing you installed
in a hurry never makes it into the declaration.

kuma gives drift a second exit. Anything this machine has that kuma.toml
doesn't name is a proposal against your declaration, and `kuma diff` says
so with both edges:

```console
$ kuma diff
packages.flatpak
  - org.gnome.Boxes  installed, not declared (convergence removes it)

  → kuma capture   keep them: declare what this machine already runs
  → kuma sync      converge now; otherwise the boot/daily run picks this up
```

`kuma capture` prints the proposal and writes nothing; `--yes` writes it,
and naming items captures only those. You review a diff of your
*declaration*, not of your system. Experiment imperatively, promote
deliberately.

**Capture never touches the machine.** It reads the machine and writes the
file, so convergence authority stays exactly where it was and a dry run is
as safe as `kuma diff`.

What it will and won't take:

- **Flatpaks and brew formulae**, which are the whole mutable edge. Brew
  offers *leaves* only: a dependency is baggage that arrived with a
  choice, not a choice.
- **Not rpm**, because there is nothing to capture. A bootc machine can't
  install one imperatively, so `[packages].rpm` is already declarative.
- **`flatpak --user` installs only when named.** Declaring one installs it
  system-wide and hands it to convergence, which changes what it is rather
  than just where it's written down.
- **Never `[user]` or `[system]`.** A password hash and machine state must
  not walk into a file that gets committed and baked into an image. That
  boundary is not a TODO.

## For agents

The self-describing principle is also an API: an agent with a shell can
operate a kuma machine without kuma-specific knowledge, because every
response names the legal next commands.

- **Probe**: `kuma --json` is the root resource: state, facts, and
  `actions` as `{rel, cmd, why}` objects. Execute an action's `cmd`
  verbatim, then re-probe. `kuma doctor --json` (machine health) and
  `kuma diff --json` (drift) carry findings with their fixes in the same
  action shape.
- **Write**: `kuma schema` prints the JSON Schema for `kuma.toml`,
  generated from the same types that parse it, field docs included, so
  it cannot drift from reality. `kuma check [--json]` validates a
  declaration without building anything.
- **Mutate**: `build`, `switch`, `update`, `rollback`, `sync`, `add`, and
  `remove` accept `--json`: stdout carries exactly one JSON document:
  `{"ok": true, ...}` with result fields and next `actions`, or
  `{"ok": false, "error": ...}` with a non-zero exit. Progress and
  subprocess output move to stderr. Mutations gate on `--yes` and never
  touch the running system: they build and stage; a reboot applies.

Without `--config`, kuma uses `./kuma.toml`, falling back to
`~/.config/kuma/kuma.toml`, a home for declarations that don't live in a
project checkout. Neither is ever created implicitly; `kuma init` is how a
declaration comes to exist, and on a kuma machine it writes a copy of the
machine's own baked declaration (`--starter` for the generic template).
With no working copy at all, the read-only commands (`update`, `diff`,
`generate`) fall back to the baked declaration itself: a machine installed
from an ISO can `kuma update --yes` without ever creating a file. Editing
(`add`, `remove`, `build`) is what requires one.

## Example config

```toml
schema_version = 1

[system]
base = "quay.io/fedora/fedora-bootc:44"
desktop = "niri"   # curated desktop sets: "niri" or "cosmic"; omit for headless

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

## Boot health and automatic rollback

Every image bakes [greenboot](https://github.com/fedora-iot/greenboot-rs).
There is nothing to configure: a declarative system whose bad update can
strand the machine isn't declarative where it counts, so the safety net is
not opt-in.

How it works: the first boot of every new deployment arms a rollback
trigger. A GRUB boot counter gives the deployment three attempts; a boot
that never reaches its health checks burns one attempt just the same, so
even a hang before userspace counts. On a desktop image, a boot is healthy
only once the greeter is actually on screen (`display-manager.service`
active within 120 s); "boots fine into a black screen" is precisely the
failure this exists to catch. When the attempts run out, GRUB falls back
to the previous deployment and greenboot makes it permanent with
`bootc rollback`. A bad update costs three reboots, not the machine.

Two deliberate choices:

- **No default health checks.** greenboot's optional check package makes
  DNS resolution a *required* check: reasonable on an always-networked
  IoT box, absurd on a laptop that boots offline. Kuma installs the core
  framework plus its own greeter check, nothing else. Drop your own
  scripts into `/etc/greenboot/check/required.d/` if you want more.
- **Existing machines are retrofitted, not abandoned.** The boot counter
  is *bootloader* config, written once at install time: a machine
  installed before boot health entered its image would count nothing, and
  a failing update would reboot-loop forever instead of falling back.
  `kuma-boot-health-sync` closes that gap on every boot: if the
  bootloader's config predates greenboot, the counter logic is converged
  into `/boot/grub2/custom.cfg` (the hook GRUB's static config already
  sources), and removed again if the bootloader ever learns to count
  natively. It runs before the first health verdict, so even the very
  first update onto a boot-health image is already protected.

A rollback isn't silent: the failed deployment stays in the rollback
slot, `kuma` reports it, and `kuma doctor` grades boot health: whether
this boot passed its checks, and whether the bootloader can actually
count attempts. An old, previously-good deployment that starts failing
(hardware, not the update) reboots three times and then waits for a
human; rolling back can't fix what an update didn't break.

## Developing and testing

- **Smoke tests** (`scripts/smoke.sh`) build every committed example and,
  with `--boot`, boot it. Three stages, cheapest first: `check` validates
  the declaration, `image` builds it and inspects the layers a successful
  build doesn't already prove (the baked declaration is byte-identical,
  the branding sed landed, greenboot is present), and `boot` makes a disk,
  boots it headless, and asks the machine whether the boot was healthy.
  That last verdict is greenboot's own: the same check that would roll an
  update back decides whether the test passed, which on a desktop image
  means the greeter came up. "Boots fine into a black screen" fails here
  instead of on your laptop.

  ```console
  $ cargo test                   # the tier that needs no machine, on every change
  $ scripts/smoke.sh             # check + image, every example
  $ scripts/smoke.sh --boot      # all three stages (needs KVM and sudo)
  $ scripts/smoke.sh --boot cosmic
  ```

  CI runs `cargo test`, clippy, and the image stage on the minimal example
  only: a desktop image doesn't fit a hosted runner's disk, and the boot
  stage needs KVM and sudo. Run `--boot` locally before pushing anything
  that touches image contents.
- **CLI development** works anywhere Rust does, including a distrobox. When
  kuma runs inside a container it drives the *host's* podman/bootc via
  `flatpak-spawn --host`.
- **Inspecting a built image**: it's a normal OCI image, so
  `podman run --rm -it localhost/kuma:latest bash` or even
  `distrobox create --image localhost/kuma:latest` work fine for poking at
  the userland.
- **Boot testing** (the real thing: systemd, kernel args, `bootc switch`,
  rollback) can't happen in a container. `kuma vm` builds a qcow2 via
  bootc-image-builder and boots it in QEMU. Log in as your declared
  `[user]` (created on first boot), or the always-present test user
  `kuma`/`kuma` (`ssh -p 2222 kuma@localhost`; your ssh key is injected
  automatically). It needs sudo: bootc-image-builder runs as root.
  After a rebuild of the image, pass `--rebuild`; kuma warns when the
  reused disk is older than the image.
- **Iterating without losing state**: `kuma vm --apply` streams the freshly
  built image into the *running* VM and `bootc switch`es inside it. The VM
  reboots into the new image with `/var` intact: flatpaks, brew, and homes
  survive, so nothing re-downloads. It's also the real update path (staged
  deployment; `bootc rollback` inside the VM undoes it). Use `--rebuild`
  only when you want a factory-fresh machine.
- **Installer media**: `kuma iso` builds an Anaconda installer ISO from the
  image (`iso/bootiso/install.iso`), bootable in GNOME Boxes or `dd`'d to a
  USB stick. The install is interactive (language, disk), but kuma-owned
  choices are preseeded: hostname, no initial-setup, and, when kuma.toml
  declares a `[user]`, no Anaconda user screen, since the declared account
  is created on first boot. Unlike `kuma vm` disks, ISOs carry no test user.
  A declared `[user]` rides into the installer (account and password hash),
  and `kuma iso` says so when it happens; build media you'll share from a
  declaration without `[user]`, and Anaconda's user screen returns.

## Roadmap

- [x] `kuma.toml` v1 schema: base, rpm packages, services
- [x] Containerfile generation + local podman build
- [x] `kuma switch` via containers-storage transport
- [x] `kuma vm`: qcow2 via bootc-image-builder, booted in QEMU
- [x] `kuma iso`: Anaconda installer ISO for real hardware and Boxes
- [x] `kuma diff`: drift between the declaration, the image, and the machine
- [x] `kuma add` / `kuma remove`: edit the declaration in place, comments intact
- [x] `kuma doctor`: deployment, convergence, GPU, and disk health checks
- [x] `kuma update`: pull the latest base, rebuild, stage in one step
- [x] Flatpaks: Flathub remote in-image, declared apps converged on boot
- [x] Declarative user: created on first boot, converged after (/home is machine state)
- [x] `kuma passwd`: hash a password for the `[user]` section
- [x] `kuma sync`: converge flatpaks and brew on demand (user config later)
- [x] `kuma vm --apply`: update the running VM in place, `/var` intact
- [x] `kuma clean`: reclaim stranded build images and abandoned build containers
- [x] `kuma rollback`: the update's undo, boot order back to the previous deployment
- [x] Bare `kuma`: the state machine as hypermedia (state + next actions, human and JSON)
- [x] `--json` across the read surface: bare `kuma`, `doctor`, `diff` speak the same map to agents
- [x] Agent surface: `kuma schema` + `kuma check`, `--json` on every mutating verb, structured errors
- [x] `desktop = "cosmic"`: second curated desktop; COSMIC curates itself, kuma adds enablement + identity
- [x] Self-describing images: the baked declaration, seeded `kuma init`, config search path
- [x] Boot health + auto-rollback: greenboot in every image; desktop boots must reach the greeter or fall back
- [x] `kuma capture`: drift as a proposal against the declaration, not an error to erase
- [x] CI build-and-boot smoke tests: every example built, booted headless, and judged by its own greenboot verdict
- [ ] `kuma.lock`: pin base digest and package versions; `kuma update` moves pins deliberately
- [ ] `kuma doctor` drift detection for `/etc`: flag local edits shadowing the image's baked defaults
- [ ] Registry publishing + CI builds (`bootc switch`-able from anywhere)
