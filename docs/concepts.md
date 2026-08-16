# How kuma behaves

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
- **`kuma update --check` asks the repos, not a tag.** A composed base has
  no upstream tag whose movement can be checked, and every package in it
  is in play, because an update recomposes the whole thing. So the check
  asks dnf what has a newer version in the repos and which of those carry
  security advisories. Seconds, and it builds nothing.

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

## The image is self-describing, not just self-contained

A bootc image is expected to be self-contained: everything the system runs
is in it. Kuma's images also carry what the machine needs to reason about
itself, which is three things.

The declaration it was built from, verbatim at `/usr/lib/kuma/kuma.toml`,
comments and formatting intact. Kuma itself, at `/usr/bin/kuma`. And the
units and helpers that converge flatpaks, brews, the declared user, and
boot health.

That is the complete set needed to answer "what am I supposed to be" and
then act on it, which means a machine needs neither a working copy of your
declaration nor a tool you brought with you. `kuma update --yes` works on a
machine installed from an ISO that has never had a `kuma.toml` anywhere,
because read-only commands fall back to the baked one. `kuma init` on such
a machine seeds a copy true to that machine rather than a generic starter.

The lock is deliberately not included. A `kuma.lock` belongs in git next to
the declaration it pins, not on every machine built from it.

Two things worth knowing about the baked declaration. It is world-readable,
because the probe and `kuma init` both need it, so a `password_hash` line in
your declaration is readable by any local user; the hash that actually
creates the account ships separately at 0600. And it records what the
machine was built to be, not what it is now. Comparing the two is what
`kuma diff` does, and what the machine has that the file does not name is a
fork rather than a fault.

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
The base is composed locally from Fedora's repos (localhost/kuma-base:m26ccdd18fd07).
20 packages have moved in the repos since this machine booted its image.
      kernel 7.1.7-200.fc44.x86_64 -> 7.1.8-200.fc44.x86_64 (important)
      sqlite-libs 3.51.2-1.fc44.x86_64 -> 3.51.2-2.fc44.x86_64 (important)
      linux-firmware 20260622-1.fc44.noarch -> 20260810-1.fc44.noarch (moderate)
      ripgrep 14.1.1-4.fc44.x86_64 -> 15.2.0-1.fc44.x86_64
      waybar 0.15.0-1.fc44.x86_64 -> 0.15.0-2.fc44.x86_64
rpm   20 moved, 16 with security advisories (5 important, 11 moderate)
```

For a named base, `--check` is one registry query and reports only whether
the base moved: a rebuild layers with `dnf install` rather than upgrading,
so the base's own packages cannot move underneath you. A composed base has
no tag to ask about and every package in play, so the check asks dnf
instead, worst advisory first.

It asks the running machine when there is one, which means it does not
care how kuma got installed and needs no image in podman storage; a host
that isn't a kuma machine gets asked about the image it builds, and the
output says which answered. Repo metadata is cached under
`~/.cache/kuma/dnf`, so the first run takes about half a minute and the
rest take seconds. No root either way.

It is a prediction. dnf reports what it would upgrade; a recompose runs
its own depsolve, which can also add or drop packages an upgrade query
never sees. The lock diff afterwards is the record of what happened.

## What updates itself, and what waits for you

Flatpaks and brew formulae converge on a daily timer: what the declaration
names gets installed, what is already there gets updated, and what
convergence installed but the declaration no longer names gets removed.
They are per-package, reversible, and live the moment they land, so there
is nothing to gain by making you ask first.

The image is the opposite on every count. An update replaces the entire OS
at once and applies on the next boot, so putting it on a timer buys either
surprise reboots or a queue of staged deployments nobody has booted while
the machine reports itself up to date. Kuma stages nothing you did not ask
for, and `kuma update` stays yours to run.

What is automated is knowing when to run it:

- `kuma update --check` asks the repos what has moved since this image was
  built, security advisories first. Seconds, and it builds nothing.
- `kuma doctor` reports how old the booted image is and warns past a
  month, which on Fedora means at least one kernel you did not take.

```console
warn  deployment: booted image is 41 days old
      → kuma update   recompose against the repos' current packages and rebuild
```

Neither one applies anything. The machine watches the clock; you decide
when to reboot into a new one.

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

The same merge has a second edge, and it decides where machine state can
live. A file that an image ships lands in `/usr/etc`, so it is not a local
modification. If a later image drops that file, the merge drops it from
`/etc` too. Anything written once and expected to outlive updates therefore
cannot arrive as image content in `/etc`.

That is why `kuma install` writes the account it asks for to
`/var/lib/kuma/user` rather than `/etc/kuma/user`. bootc fills `/var` from
the image once, at install, and never touches it again, which is exactly
what install-time answers need. The hostname has the same problem and the
opposite cure: `/etc/hostname` *is* image content, so the installed machine
writes it at first boot from `/var/lib/kuma/hostname`, and writing it is
what makes it a local modification and therefore what survives.

So the rule has three parts, not two: `/usr` is the image, `/etc` is
machine state the image also has an opinion about, and `/var` is machine
state it does not.

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


## A disk is not a declaration

Everything above is about images. An install is where an image meets a
machine, and the two know different things.

An image knows what should be installed. It cannot know who the machine is
for: a published image is pulled by strangers, so it declares no `[user]`,
and a machine installed from one would otherwise have no account and no way
in. So `kuma install` asks, writes the answers to `/var/lib/kuma/user` on the
target, and `kuma-user-sync` creates the account at first boot exactly as it
does for a declared one. The installer creates nobody; it writes down what
the machine should converge to.

That split decides where things live. `/var` is filled from the image once,
at install, and never touched again, which is what install-time answers need.
`/etc` is merged against the image on every update, so a file the installer
shipped there would be image content rather than a local change, and the next
update would delete it. The hostname is the exception that proves it:
`/etc/hostname` *is* image content, so the installed machine writes it at
first boot from `/var/lib/kuma/hostname`, and writing it is what makes it
local and therefore what makes it survive.

The disk itself is machine state too. Kuma writes the same three partitions
every time: an EFI system partition, a `/boot` outside the root, and a root
that takes the rest. `/boot` is separate even when nothing is encrypted, so
that turning encryption on changes what the third partition holds rather than
the shape of the disk. Whether it holds a LUKS container is asked at install
and cannot be revised afterwards without installing again, which is why it is
asked before the plan is printed rather than defaulted either way.

A first boot then spends minutes on the rest of it. The account is made,
the hostname applied, and the declared flatpaks and brews downloaded,
which for a full desktop is about a gigabyte and takes a few minutes on
an ordinary connection. Kuma says so while it happens: bare `kuma` reports
`converging` rather than drift, and offers no `sync`, because a sync is
what is running. A machine that does not match its declaration yet is not
the same as one that has stopped trying.

None of that is in `kuma.toml`, and none of it should be. Two machines
installed from one declaration can have different disks, different names, and
different people. The declaration says what the system is; the install says
whose it is.
