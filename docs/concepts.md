# How kuma behaves

The reasoning behind the parts of kuma that are not obvious from the
verbs. None of this is needed to use it; all of it explains why it does
what it does.

## Drift is a fork, not an error

Declarative systems normally treat drift as failure: the machine deviates,
the tool corrects it, the deviation is erased. That is why the thing you
installed in a hurry never makes it into the declaration.

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
if you like: being undeclared costs reproducibility, never survival.

Membership and currency are separate questions, and only the first belongs
to the declaration. It decides what exists. Keeping software current is
kuma's job regardless of who installed it, so a store-installed app, an
ad-hoc `brew install`, and a flatpak runtime are updated on the same
schedule as a declared app. Nothing is left to rot for the crime of not
being written down. Each ecosystem's own hold still works if you want to
stay on a version: `flatpak mask` and `brew pin`.

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

## The base is composed, not inherited

The usual way to build a bootc image is `FROM` a general-purpose base,
then remove what you didn't want. With no `system.base` in the
declaration, kuma instead composes its own with `rpm-ostree compose
image`, the same tool and building blocks Fedora uses to build
fedora-bootc.

The compose starts from Fedora's minimal bootc manifest, whose summary is
"effectively just bootc, systemd, kernel, and dnf as a starting point",
and adds what a real machine needs. What fedora-bootc carries for the
general case is never included rather than removed afterward. Fedora stays
the package source: kuma builds no packages and no kernels, and every
version comes from Fedora's repos at compose time.

The composed image is content-addressed. Its tag embeds a hash of the
manifest that produced it, so a build can name its base before any compose
has run, an unchanged manifest reuses the image already in storage, and a
changed manifest cannot reuse a stale one.

Two consequences:

- **`system.firmware` is the trim.** Unset, the base ships every vendor's
  firmware, so a machine that declares nothing about its hardware still
  boots with working GPU, wifi, and audio. Name what your hardware needs
  and the rest stays out of the image.
- **`kuma update --check` has no tag to ask about.** A composed base has
  no upstream tag whose movement can be checked, and the repos underneath
  it move continuously. It reports that rather than a "current" it cannot
  establish; `kuma update` recomposes against the repos as they are now,
  and the lock diff shows what moved.

Naming a `base` opts out of all of it: any bootc image can be one.

**The base runs sshd.** `openssh-server` is composed in, and the image
enables `sshd.service` by name, so every kuma machine listens on port 22
and firewalld's default zone permits it. This is deliberate rather than
inherited: `kuma vm` and the boot stage of the smoke tests both reach a
guest over ssh, so an image that could not be reached that way would take
the test harness with it.

Authentication is Fedora's default, which means passwords work. If you
declare `[user].ssh_keys`, kuma serves them from `/etc/kuma/keys/<name>`
alongside the user's own `~/.ssh/authorized_keys` and never overwrites
it. To require keys, drop a conf into `/etc/ssh/sshd_config.d/`; `/etc`
is merged rather than replaced, so it survives image updates, and `kuma
doctor` will report it as a local modification because it is one.

`[services].disable = ["sshd.service"]` turns it off on a machine that
doesn't want it. It is a default, not part of kuma's floor: the image
enables it above your `[services]` block, so your declaration wins, the
way it does for anything a desktop enables. Boot health and rollback sit
below that line and cannot be switched off. Disable sshd and `kuma vm`
still builds a disk, but nothing will be able to ssh into it.

## A desktop is infrastructure, your declaration is applications

Choosing a desktop installs packages you did not name. That set is session
infrastructure: the parts that have to exist for a session to function.
Applications are not in it, even convenient ones, because the two are
reversible in different ways. Delete a line from your declaration and the
next convergence uninstalls it; a package in a desktop set has no opt-out,
so putting one there is a decision you make on behalf of everyone.

You can always add with `packages.rpm`. You cannot subtract from the set.
[What a desktop contains](desktops.md) lists both arms, explains the
non-obvious members, and says where that limit bites.

## kuma.lock

`base = "…/fedora-bootc:44"` names a tag, and tags move. One such move,
bootc 1.16.6 to 1.16.7 between two updates, is enough to break every build
that trusted the tag.

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

A composed base has no tag to distrust, so the lock holds its
content-addressed reference instead. A changed manifest then reads as a
changed base, as an edited `system.base` would. The pin holds while that
image is still in storage; once it isn't, kuma says so and composes a
fresh one.

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

For a named base, `--check` is one registry query: no pull, no build. It
reports only whether the base moved, because that is the only question
with an honest cheap answer. For a composed one there is no such question,
and it says so instead.

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
That is how the display fix that motivated this check got resolved: the
workaround stopped being a local edit and became something kuma bakes.

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

