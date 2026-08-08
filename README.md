# Kuma

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
$ kuma doctor                              # machine health, /etc drift, GPU, disk
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

## Drift is a fork, not an error

Declarative systems normally treat drift as failure: the machine deviates,
the tool corrects it, the deviation is erased. That is why imperative
escape hatches always feel like cheating, and why the thing you installed
in a hurry never makes it into the declaration.

Kuma gives drift a second exit. Anything the machine has that `kuma.toml`
doesn't name is a proposal against your declaration:

```console
$ kuma diff
packages.flatpak
  - org.gnome.Boxes  installed, not declared (convergence removes it)
Ad-hoc flatpaks, kept as yours: io.github.kolunmi.Bazaar

  → kuma capture   keep them: declare what this machine already runs
  → kuma sync      converge now; otherwise the boot/daily run picks this up
```

Snapshots follow the same rule. `kuma snapshot --restore <path>` is a dry
run that names which snapshot the path would come back from and whether a
copy on the machine gets replaced; `--yes` does it. It restores a path,
never a whole subvolume: swapping what `/var/home` *is* while processes
hold files open in it is a reboot-shaped operation, and the accident
people actually have is one file.

Convergence takes back only what it installed. Boxes above was declared
once and no longer is, so it is on the removal list; Bazaar you installed
yourself, so it is undeclared but in no danger. Install apps from a store
if you like — being undeclared costs reproducibility, never survival.

`kuma capture` prints the proposal and writes nothing until `--yes`; naming
items captures only those. You review a diff of your *declaration*, not of
your system. Experiment imperatively, promote deliberately.

Capture never touches the machine, only the file, so a dry run is as safe
as `kuma diff`. It takes flatpaks and brew leaves, which are the whole
mutable edge. It will not take rpms, because a bootc machine can't install
one imperatively and `[packages].rpm` is already declarative. It takes a
`flatpak --user` install only when you name it, since declaring one makes
it system-wide. And it never touches `[user]` or `[system]`: a password
hash and machine state must not walk into a file you commit.

## kuma.lock

`base = "…/fedora-bootc:44"` names a tag, and tags move. One moved bootc
1.16.6 to 1.16.7 between two updates here and broke every build.

`kuma.lock` appears beside your declaration after the first build. There is
no verb to learn: `kuma build` reads it and refreshes it, and `kuma update`
is the one thing that moves the pin. Commit it.

What it pins and what it merely records is a deliberate split:

- **The base digest is enforced.** Builds resolve `FROM name@sha256:…` from
  the lock, so the same declaration plus the same lock builds from the same
  bytes anywhere.
- **Package versions are recorded, not pinned.** Fedora's mirrors garbage
  collect old builds within weeks, so a version pin becomes a build failure
  that has nothing to do with your declaration. The record exists to be
  diffed, which needs no enforcement, so a lock can never break a build.

That makes an update legible, and `git diff kuma.lock` is the full story:

```console
$ kuma update
base  sha256:9f3ca81b2e4d -> sha256:a71b04ef9c33
      bootc 1.16.6-1.fc44.x86_64 -> 1.16.7-1.fc44.x86_64
      ... and 34 more changed
rpm   36 changed, 2 added

$ kuma update --check
quay.io/fedora/fedora-bootc:44 is current (sha256:1650030cbdb1).
```

`--check` is one registry query: no pull, no build. It reports only whether
the base moved, because that is the only question with an honest cheap
answer.

## /etc is merged, not replaced

On an ostree system, every difference between your `/etc` and the image's
defaults in `/usr/etc` is treated as a local modification and carried onto
every future deployment. A file you edit by hand keeps winning, silently,
no matter what later images ship.

That is working as designed, and it is a trap. You fix something by editing
`/etc` directly, later declare the same fix properly, and the hand-edited
copy goes on overriding it. The declared version can never be tested, and
nothing tells you why.

`kuma doctor` watches the files your image owns:

```console
ok    etc: 14 files this image owns in /etc, none shadowed locally

warn  etc: local edits shadow the image: /etc/environment. These win over
      every future image, so the declared version never applies
      → sudo cp /usr/etc/environment /etc/environment
```

The cure is `cp`, not `rm`: a deletion is itself a local modification and
carries forward as one.

There is deliberately no `kuma capture` for this. Package drift is a fork
because a package is your choice; `/etc` content is kuma's curation, so an
edit worth keeping belongs in the image rather than in your declaration.
That is exactly how the display fix that motivated this check got resolved:
the workaround stopped being a local edit and became something kuma bakes.

## Boot health and automatic rollback

Every image bakes [greenboot](https://github.com/fedora-iot/greenboot-rs).
There is nothing to configure, because a declarative system whose bad
update can strand the machine isn't declarative where it counts.

The first boot of a new deployment arms a rollback trigger, and a GRUB boot
counter gives it three attempts. A boot that hangs before userspace burns
an attempt just the same. On a desktop image, a boot counts as healthy only
once the greeter is actually on screen, so "boots fine into a black screen"
is precisely what this catches. When the attempts run out, GRUB falls back
and greenboot makes it permanent. A bad update costs three reboots, not the
machine.

Two deliberate choices:

- **No default health checks.** greenboot's optional check package makes DNS
  resolution *required*: reasonable on an always-networked IoT box, absurd
  on a laptop that boots offline. Kuma installs the core framework and its
  own greeter check. Add your own under
  `/etc/greenboot/check/required.d/`.
- **Existing machines are retrofitted.** The boot counter is bootloader
  config written once at install time, so a machine installed before boot
  health entered its image would count nothing and reboot-loop forever
  instead of falling back. `kuma-boot-health-sync` converges that on every
  boot, and removes it again if the bootloader learns to count natively.

A rollback isn't silent: the failed deployment stays in the rollback slot,
and `kuma doctor` grades both this boot's verdict and whether the
bootloader can actually count. A previously-good deployment that starts
failing reboots three times and then waits for a human, because rolling
back can't fix what an update didn't break.

## For agents

The self-describing principle is an API. An agent with a shell can operate
a kuma machine without kuma-specific knowledge, because every response
names the legal next commands.

- **Probe.** `kuma --json` is the root resource: state, facts, and `actions`
  as `{rel, cmd, why}`. Execute an action's `cmd` verbatim, then re-probe.
  `doctor --json` and `diff --json` carry findings with their fixes in the
  same shape.
- **Ask before doing.** `check --json` validates a declaration,
  `update --check --json` reports whether the base moved, `diff --json`
  reports drift. All three change nothing.
- **Write.** `kuma schema` prints the JSON Schema for `kuma.toml`, generated
  from the same types that parse it, so it cannot drift from reality.
- **Mutate.** `build`, `switch`, `update`, `rollback`, `sync`, `add`,
  `capture`, `remove`, and `clean` accept `--json` and emit exactly one
  document on stdout: `{"ok": true, …}` with result fields and next
  `actions`, or `{"ok": false, "error": …}` with a non-zero exit. Progress
  and subprocess output move to stderr.
- **Nothing changes what's running without a reboot.** The verbs that touch
  the system (`switch`, `update`, `rollback`) gate on `--yes` and even then
  only stage a deployment.

Without `--config`, kuma reads `./kuma.toml`, falling back to
`~/.config/kuma/kuma.toml`. Neither is ever created implicitly. With no
working copy at all, read-only commands fall back to the machine's baked
declaration, so an ISO-installed machine can `kuma update --yes` without
ever creating a file; editing is what requires one.

## Developing and testing

**Smoke tests.** `scripts/smoke.sh` builds every committed example and, with
`--boot`, boots it. Three stages, cheapest first: `check` validates the
declaration, `image` builds it and inspects what a successful build doesn't
already prove, and `boot` makes a disk, boots it headless, and asks the
machine whether the boot was healthy. That last verdict is greenboot's own,
so the check that would roll an update back is the one that decides whether
the test passed.

```console
$ cargo test                    # the tier that needs no machine
$ scripts/smoke.sh              # check + image, every example
$ scripts/smoke.sh --boot       # all three stages (needs KVM and sudo)
$ scripts/smoke.sh --boot cosmic
```

CI runs `cargo test`, clippy, and the image stage on the minimal example:
a desktop image doesn't fit a hosted runner's disk, and the boot stage needs
KVM. Run `--boot` locally before pushing anything that touches image
contents.

**Booting a VM.** `kuma vm` builds a qcow2 via bootc-image-builder and boots
it in QEMU (it needs sudo; bootc-image-builder runs as root). Log in as your
declared `[user]`, or the always-present test user `kuma`/`kuma`
(`ssh -p 2222 kuma@localhost`; your ssh key is injected). Pass `--rebuild`
after rebuilding the image; kuma warns when the reused disk is older.

**Iterating without losing state.** `kuma vm --apply` streams the freshly
built image into the *running* VM and switches inside it. `/var` survives,
so flatpaks, brew, and homes don't re-download. It's also the real update
path, so `bootc rollback` inside the VM undoes it.

**Installer media.** `kuma iso` builds an Anaconda installer ISO
(`iso/bootiso/install.iso`), bootable in GNOME Boxes or `dd`'d to a USB
stick. Kuma-owned choices are preseeded; the rest is interactive. A declared
`[user]` rides into the installer, and `kuma iso` says so when it happens,
so build shareable media from a declaration without one.

**Inspecting an image.** It's a normal OCI image:
`podman run --rm -it localhost/kuma:latest bash`.

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
