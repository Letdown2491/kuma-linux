# Changelog

## Unreleased

Entries land with the change they describe; the next tag takes this section
as its release notes.

## v0.5.0 (2026-08-13)

The machine notices when its own bytes went stale. Taking a new kernel is
still something you ask for.

### Behavior

- `kuma update --check` reports every package that has moved, for a composed
  base. It asks dnf which installed packages have a newer version in the repos
  and which of those carry security advisories, then prints them worst first
  with a `20 moved, 16 with security advisories (5 important, 11 moderate)`
  summary. Seconds, and it builds nothing. Previously a composed base had no
  cheap question at all and the check could only say so. A declared base still
  reports whether its tag moved, because a rebuild layers rather than upgrades
  and its packages are not in play.
- The check asks the running machine when there is one, so it does not care
  whether kuma arrived by ISO, `kuma switch`, or a rebase, and needs no image
  in podman storage. A host that is not a kuma machine is asked about the image
  it builds instead. The output names which of the two answered.
- Repo metadata for that check is cached under `~/.cache/kuma/dnf`, about
  140MB. The first run fills it and takes roughly half a minute; later runs
  re-check freshness and answer in a few seconds. Nothing needs root: the
  default dnf state directory would have, and a check that prompts for a
  password is a check nobody runs.
- `kuma doctor` reports how old the booted image is and warns past 30 days,
  which on Fedora means at least one kernel you did not take. Nothing applies
  an update on a schedule: an image update replaces the whole OS and lands on
  the next boot, so it stays a decision. A machine with a newer deployment
  already staged is told to reboot rather than warned twice.
- Bare `kuma` reports when an image was built by a different kuma than the one
  running. Images record their builder in the `io.kuma.builder` label, and an
  image without the label counts as different, so this fires on machines built
  before it existed. Previously a machine whose declaration had not moved read
  as `in-sync` no matter how old the binary that built its image was.

### Fixed

- Anaconda writes a `/` line into `/etc/fstab` describing the root as the
  filesystem it installed onto. On a bootc machine the root is a composefs
  overlay, so `systemd-remount-fs` failed on every boot of an ISO-installed
  machine. Images now carry `kuma-fstab-sync`, which comments that line out
  when the kernel reports the root as an overlay and does nothing otherwise. A
  machine installed today fails the unit once and is clean on every boot after.
  `kuma doctor` no longer excuses the failure once the cause is gone.

## v0.4.0 (2026-08-09)

A machine can run kuma, and its disks are built on ext4.

### Behavior

- Every image ships `/usr/bin/kuma`: the binary that built it, copied in rather
  than downloaded. A machine installed from a 0.3.0 image had the baked
  declaration, the convergence units, and the helpers, but nothing to run them,
  so `kuma update --yes` on an ISO-installed machine was a documented promise
  with no binary behind it.
- `kuma vm` disks are built on ext4 rather than xfs. A disk from 0.4.0 is a
  different filesystem than one from 0.3.0. Nothing migrates and nothing needs
  to, since `kuma vm --rebuild` makes the new one.
- Images name `sshd.service` rather than inheriting it from Fedora's preset.
  Behavior is unchanged, since sshd was already enabled on every kuma machine.
  What changes is that `services.disable` can turn it off, and an upstream
  preset change can no longer quietly alter what a kuma machine exposes.
- The example declarations dropped LibreOffice, Bazaar, and org.gnome.Firmware,
  and are renamed to `niri.toml`, `cosmic.toml`, and `minimal.toml`. Rebuilding
  from an updated example takes those three flatpaks back off the machine,
  because convergence removes what it installed. Add one back with
  `kuma add --flatpak`.

### Fixed

- One `kuma vm` build left udisks2 mounts holding a loop device open, and every
  later build from the same declaration then failed on a duplicate filesystem
  UUID, forty lines into an osbuild traceback that named neither the loop
  device nor the mount. ext4 permits duplicate UUIDs, so the collision can no
  longer fail a build, and `kuma vm` names any stale mounts it finds along with
  the commands that clear them.
- `kuma vm` on a host with no ssh key built a VM reachable only by password.
  It generates an ed25519 throwaway into the VM output directory instead, and
  reuses one already sitting there rather than locking out disks beside it. The
  launch message now names the key that will actually work.
- An ISO built from the shipped example installed a machine nobody could log
  into: declaring a `[user]` removes Anaconda's create-a-user screen, and with
  no password hash the account was created locked. The examples no longer
  declare a user, which makes them directly usable as shareable media.

## v0.3.0 (2026-08-08)

The download URL was still serving convergence that let packages rot.

### Behavior

- Convergence updates everything on the machine, not just what the declaration
  names. Both syncs previously upgraded only the declared list, so an
  undeclared flatpak, a brew cask, or a runtime no declared app demanded was
  never updated by anything, on a machine running convergence daily. `brew
  upgrade` and `flatpak update --system` now run without an argument list.
  This takes no authority kuma did not have: membership still comes from the
  declaration, removal still reaches only what convergence installed, and
  `flatpak mask` and `brew pin` still hold a package where it is.

  v0.3.0 exists mainly to get this to the front door. `releases/latest/download/`
  resolves to the newest non-prerelease, so it was still handing out v0.2.0,
  and a machine built from that binary looks healthy while nothing it installs
  outside the declaration ever updates.

## v0.2.0 (2026-08-08)

Getting kuma needed a compiler, and the binary could not say which one it was.

First tagged release. Everything before it is in the git log.

### Behavior

- Kuma is published as a static `x86_64` binary that needs nothing installed
  alongside it, which is the point on the image-based machines most likely to
  want it: podman and no toolchain.
- Every release asset is signed with Sigstore and carries one bundle, verified
  with `cosign verify-blob --bundle`. See [SECURITY.md](SECURITY.md).
- A rolling `latest` prerelease tracks `main` between releases.
- `kuma --version` reports the commit it was built from, and appends `-dirty`
  when that tree had uncommitted changes.
