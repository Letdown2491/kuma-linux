# Changelog

## Unreleased

Entries land with the change they describe; the next tag takes this section
as its release notes.

### Added

- `kuma install` says when the image it is installing declares an account
  that is not the one being created. A shared image declares none, which
  is why the installer asks; an image built from somebody's own
  declaration carries their name and password hash in its baked
  declaration, onto a disk for an account it will never create. `kuma iso`
  already warned about the same hazard riding into installer media.
- Bare `kuma` says `converging` while a sync unit is running, instead of
  reporting drift and offering a `kuma sync` that is already underway. A
  first boot spends minutes installing declared apps, and for all of it the
  machine genuinely does not match its declaration: read as a snapshot that
  is drift, read in time it is progress. `kuma doctor` says a unit is
  running now rather than that its last run succeeded, since systemd
  reports `Result=success` for a run that has not finished.
- `kuma doctor` says whether the root is encrypted. Nothing else on a
  running machine reports it, both answers are legitimate, and the choice
  cannot be revised without reinstalling, so it is stated rather than
  graded.
- `scripts/smoke.sh --install` writes a real encrypted disk and verifies
  what landed on it: that the container opens with the passphrase that was
  typed, that a boot entry unlocks the container actually present, that the
  account file names the account the installer was given, and that no
  greeter autologins somebody the disk has no account for. It needs sudo
  and no KVM, and is separate from `--boot` because it asks a different
  question: not whether a machine works, but whether the disk is the one
  that was described.
- `kuma install` can encrypt the disk. It asks on a terminal, takes
  `--encrypt` from anything else, and is off unless something says
  otherwise. The passphrase reaches cryptsetup on a pipe and is never a
  flag, a file, or an argument `ps` could show; the root partition holds
  a LUKS2 container with the same btrfs root inside it, and the installed
  machine unlocks it from a kernel argument at every boot. `/boot` was
  already a partition of its own for exactly this, so encrypting changes
  what the third partition holds and nothing about the shape of the disk.

### Fixed

- `scripts/smoke.sh` pulls a declared base before building it. A base
  already in local storage is the one podman builds on however old it is,
  so the lock recorded that digest while `update --check` asked the
  registry, and the two disagreed the moment Fedora pushed a new base.
  The harness read that true answer as the false alarm it was written to
  catch. CI never met it, because a fresh runner has nothing local to be
  stale. The failure also now reports what `update --check` said.
- `kuma install` syncs a locally built image into root storage before
  installing it. The install script runs as root, whose podman store is not
  the one a rootless `kuma build` writes to, and nothing bridged them.
  Where an earlier `kuma vm` had left a copy in root's storage the *stale*
  copy was installed instead of the image just built; where it had not, the
  build fell through to `docker://localhost/...` and failed on a refused
  connection. `switch`, `vm` and `iso` already did this; `install` was the
  one root-side path that did not.
- Installing an image whose declaration sets `autologin` no longer produces
  a machine with no greeter. The image bakes that account's name into
  greetd's `initial_session`, and an install creates the account it was
  asked for instead, so the greeter tried to log in a user that did not
  exist: `pam_acct_mgmt: USER_UNKNOWN`, five restarts, then nothing to log
  in at. The install layer now drops `initial_session` when the account it
  creates is not the one the image declares, in the niri config and the
  COSMIC one, and keeps it when they match. Autologin belongs to the
  account that declared it, the same rule `kuma-user-sync` already applies
  to a password.
- The greeter health check no longer passes a greeter that is crash
  looping. It sampled `display-manager.service` once, and a unit with
  `Restart=` is briefly active on every retry, so a machine nobody could
  log in to reached `greenboot-success.target` in 15 seconds and the
  greeter gave up four seconds later. It now requires the greeter to still
  be up a moment after it comes up, and treats a unit that has exhausted
  its restarts as failed immediately rather than polling a corpse for two
  minutes.
- Installing to a disk image says what it did to this machine's firmware.
  bootc writes an EFI boot entry naming the ESP it just installed, which
  is right for a disk and pollution for a file: the entry sorts itself
  ahead of the entries that can boot and points at a partition inside an
  image. It cannot be prevented (`--generic-image` skips the firmware but
  also installs BIOS grub, which this layout has no partition for), so a
  file install now compares `efibootmgr` before and after and prints the
  `sudo efibootmgr -b <num> -B` that removes what it added.
- The command `kuma install` prints for confirming a dry run carries
  `--update-from`, so installing a local image no longer offers a next
  step that the next run refuses.

### Behavior

- Bare `kuma` counts a flatpak as pending removal only when kuma installed
  it, which is the rule convergence has followed since it stopped taking
  back what it never installed. It was still counting every undeclared app,
  so a machine with anything from a store reported drift that `kuma diff`
  correctly called yours, and the two disagreed in the direction that makes
  somebody think their apps are about to be uninstalled. The brew half had
  always been right; both now read one rule.

## v0.7.0 (2026-08-15)

Kuma installs itself, from media it made, onto a disk it partitioned.

### Behavior

- `[system].shell` declares the login shell accounts on a machine get. It
  exists because `[user].shell` describes a person, and shareable media
  declares no person: an image could install fish and have no way to say to
  use it, so the first machine installed from one came up with fish present
  and bash configured. A machine installed from that image inherits it
  without the installer having to read the image, because `kuma-user-sync`
  now sources the baked account file first and the installer's file second,
  taking each key from the later one. `--shell` overrides it, and a shell
  the image does not install fails the build rather than the login, which
  is the guard a declared user's shell has always had.
- `kuma install` refuses a `localhost/` image, and gained `--update-from`
  for the case that refusal would otherwise block. The installed machine
  records what it was installed from as where updates come from, and
  `localhost` there means the machine itself, which has no registry: it
  installs fine, boots fine, works fine, and fails the first time it is
  asked to take a new image. Installing a local build while tracking a
  published tag is a real thing to want, so it is now spelled out rather
  than stumbled into.
- The composed base ships `ncurses`. `ncurses-base` is terminfo and
  `ncurses-libs` is the library, and neither owns `/usr/bin/clear`, so a
  desktop with a terminal had no `clear`, `tput` or `reset`.
- `kuma install` partitions the disk itself rather than handing it to
  `bootc install to-disk`, and the plan prints the layout it is about to
  write: a 600M ESP, a 2G `/boot`, and the rest as a btrfs root holding two
  subvolumes. Two things follow from owning it. `/boot` is outside the root
  even unencrypted, because GRUB reads a kernel before anything is
  unlocked, so passphrase LUKS becomes a change to what lives inside the
  third partition rather than a different disk shape somebody would have to
  reinstall into. And the container store goes on the target instead of in
  memory: installing from live media used to pull the image into a
  RAM-backed overlay, which failed on an 8G machine and on a 10G one, and
  now needs no more RAM than booting the media does.
- A live session now offers `kuma install` as its one affordance, and the
  ISO no longer says installing from it is impossible. Both were true when
  written: there was no published image to pull, so an empty action list
  and a stated absence beat an affordance that fails. There is one now.
- A disk smaller than 16G is refused before anything is asked, with the
  arithmetic that makes it too small, and so is a machine missing any of
  the tools the install needs, named with the package that carries each
  one. Both are objections, and objections come before the interview: the
  alternative is a wiped partition table and `command not found` one typed
  password later, which is how this check came to exist.
- The line `kuma install` prints for booting a disk image now names a
  3D-capable device, which is the difference between a desktop and a black
  screen. niri allocates through GBM, and `-vga std` is display-only: the
  greeter is text on a VT and comes up on anything, so the failure arrives
  after a correct username and password and looks like a broken install.
  `kuma vm` had this right and the message did not, so both now read from
  one place.
- `kuma install` takes a file as its target, installing to a disk image
  through a loopback device. Producing a disk image is worth doing on its
  own, and it is also how the installer gets exercised end to end on a
  machine with no spare disk.

## v0.6.0 (2026-08-14)

Media somebody else can boot, and a machine somebody else can log into.

### Behavior

- `kuma install` installs kuma onto a disk, with no arguments required. The
  account is the whole difficulty it solves: a published image declares no
  `[user]`, because the image is shared and the person is not, so a machine
  installed from one has no account, no root password, and no way in.
  Anaconda's create-a-user screen used to cover that and live media has no
  Anaconda. So the installer asks, writes `/var/lib/kuma/user` on the target, and
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
  podman rather than of a registry so a dry run stays offline. With `--yes`
  it then checks the image is actually reachable before asking anything,
  because podman only discovers a missing image when the build reaches out,
  which is one typed password and a bare `exit status 125` later.
- Whole-disk only, and destructive. `bootc install to-disk` owns the
  partitioning, so there is no cryptsetup and no custom layout yet;
  passphrase LUKS needs `to-filesystem` and kuma owning the storage. Dry run
  by default like every kuma verb that changes something, and unlike the
  others this one cannot be undone: no staged deployment to discard, no
  rollback slot. It refuses a disk with anything in use on it, asking `lsblk`
  rather than only `/proc/mounts`, because the mount table names the mapper
  device for an encrypted root and a fully encrypted disk would otherwise
  look idle while running.
- `kuma-user-sync` now reads `/var/lib/kuma/user` in preference to the baked
  `/usr/lib/kuma/user`, and ships in every image rather than only when an
  account is declared. On a personal image the account is declaration and
  gets baked; on a shared one it cannot be, because the image is shared and
  the person is not. It is a no-op with neither file present.
- `/var` rather than `/etc`, and the difference is not cosmetic. bootc fills
  `/var` from the image once at install and never touches it again, while
  `/etc` is three-way merged on every update. A file an installer ships as
  image content is not a local modification, so merging against a published
  image that has no such file deletes it: the account would have outlived the
  file describing it, and the converger would have quietly stopped
  maintaining groups and shell. The hostname has the same problem and the
  opposite fix, since `/etc/hostname` *is* image content: it is written at
  first boot from `/var/lib/kuma/hostname`, which is what makes it a local
  modification and therefore what survives.
- `kuma install` asks for a hostname too. A published image cannot know one,
  so every machine installed from one would otherwise answer to the same
  name.
- Without `--yes` it describes what `--yes` will ask for rather than asking.
  A dry run that collected a name and a password and threw them away would
  look like an install right up until it silently was not one.
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
