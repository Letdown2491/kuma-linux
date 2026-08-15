# Changelog

## Unreleased

Entries land with the change they describe; the next tag takes this section
as its release notes.

## v0.6.0 (2026-08-14)

Media somebody else can boot, and a machine somebody else can log into.

### Behavior

- `kuma install` installs kuma onto a disk, with no arguments required. The
  account is the whole difficulty it solves: a published image declares no
  `[user]`, because the image is shared and the person is not, so a machine
  installed from one has no account, no root password, and no way in.
  Anaconda's create-a-user screen used to cover that and live media has no
  Anaconda. So the installer asks, writes `/etc/kuma/user` on the target, and
  `kuma-user-sync` creates the account at first boot exactly as it does for a
  declared one. The installer does not create users; it writes down what the
  machine converges to.
- Getting that file onto the target needs no post-install mounting. `bootc
  install` copies the filesystem of the container it runs inside, so kuma
  derives a one-layer image carrying the file and installs from that, while
  `--target-imgref` records the published image as what the machine fetches
  for later updates. The installed system has an account and still tracks the
  public tag.
- Run it with no `--disk` and it lists the disks it found and asks. Disks
  with anything mounted on them stay on the list, marked and refused, rather
  than being hidden, because hiding a disk makes somebody look for it among
  the ones that are left. It never picks for you, not even when exactly one
  disk is free: a single-candidate machine is the most likely place for that
  one disk to be the one you are running from. The flags remain for
  scripting, but a verb whose only entry point is a device path is one
  nobody can walk through, and an affordance that reads `kuma install --disk
  /dev/???` is not a move anyone can take.
- `--image` defaults to the published image for the same reason. Requiring
  it left the verb unusable from the one place it exists for: somebody on
  live media has no way to know a registry path, and `kuma install` on its
  own answered with a flag error. The plan says whether that image is in
  local storage or will be pulled when you confirm, which is a question
  worth answering before the step that destroys a disk, and it is asked of
  podman rather than of a registry so a dry run stays offline.
- Whole-disk only, and destructive. `bootc install to-disk` owns the
  partitioning, so there is no cryptsetup and no custom layout yet;
  passphrase LUKS needs `to-filesystem` and kuma owning the storage. Dry run
  by default like every kuma verb that changes something, and unlike the
  others this one cannot be undone: no staged deployment to discard, no
  rollback slot. It refuses a disk with anything in use on it, asking `lsblk`
  rather than only `/proc/mounts`, because the mount table names the mapper
  device for an encrypted root and a fully encrypted disk would otherwise
  look idle while running.
- `kuma-user-sync` now reads `/etc/kuma/user` in preference to the baked
  `/usr/lib/kuma/user`, and ships in every image rather than only when an
  account is declared. `/usr` is the image and `/etc` is machine state, the
  line kuma draws everywhere else: on a personal image the account is
  declaration, on a shared one it cannot be. It is a no-op with neither file
  present.
- The composed base ships firmware for Intel wifi and SOF audio, which it
  never had. The set was curated with `dnf repoquery --recommends
  linux-firmware`, which returns 13 packages and which the list matched
  exactly: the transcription was right and the method was wrong, because
  nothing recommends `iwlwifi` or `alsa-sof` at all. A typical Intel laptop
  booted a kuma image with no wireless and no sound, and since installing
  pulls over the network, could not install either. Adds five packages for
  103 MiB.
- `kuma iso --live` builds live installer media in which the image is its own
  installer environment. The existing Anaconda ISO carries two root
  filesystems that share nothing, Fedora's installer and kuma's image, which
  measured about 2.4 GB against GitHub's 2 GB cap on a release asset. The
  live ISO carries one. It boots to a desktop as `liveuser` so hardware can
  be tried before anything is written to a disk, and it needs no sudo,
  because nothing in the path runs bootc-image-builder.
- It is media for trying kuma, not yet for installing it. Carrying no second
  copy of the image means installing pulls the image the media was built
  from, and kuma publishes none, so there is no install path from it today.
  `kuma` says so in the live session rather than offering a command that
  would fail, and gains a `live` state for the purpose: the classifier used
  to read installer media as a build workspace and report `in-sync, nothing
  pending` on the one medium where something is.
- The live session runs SELinux permissive, because a container image's real
  file labels are not reachable through any podman mount; an installed
  machine is labelled by bootc and is enforcing from its first boot.
- `kuma doctor` learned the same distinction. The live layer masks the
  convergence timers on purpose, and doctor grades a machine on those timers
  running, so it reported deliberate design as failed checks. It now
  separates installer media, a booted kuma machine, and a kuma image that is
  not booted as a deployment, which also stops a `podman run` of the image
  from failing checks it was never going to pass.
- Live media carries no identity: its own `liveuser` rather than the
  declaration's account, and its own hostname rather than the one baked from
  `system.hostname`, which otherwise greeted a stranger with the name of the
  machine that built the media.
- The live ISO is UEFI-only. BIOS boot needs an El Torito image built with
  grub2-mkimage, and shipping the i386-pc modules without one produces media
  that carries BIOS machinery and still cannot boot a BIOS machine.
- `kuma iso` without `--live` is unchanged.

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
