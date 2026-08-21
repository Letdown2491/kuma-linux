# Changelog

## Unreleased

Entries land with the change they describe; the next tag takes this section
as its release notes. Say what changed and what a reader has to do
differently. Why it changed belongs in the commit that made it.

### Added

- **Kuma machines can hibernate.** Every machine's swap was zram, which is
  memory, so there was never anywhere to write a hibernate image. Kuma can now
  make a swapfile on the root disk and set the `resume=` and `resume_offset=`
  kernel arguments that resume from it.

- **`kuma install` asks**, after the encryption question, and creates the file
  before it pulls the image. Off unless you say yes. `--swap 16G` answers early
  and `--swap none` declines without being asked. On a disk you chose not to
  encrypt, the install plan says that hibernating writes the contents of memory
  to it in the clear.

- **`kuma hibernate`** does the same on a machine that is already running, so
  this needs no reinstall. It defaults to the size of memory, prints what it
  would do, and changes nothing without `--yes`. `--off --yes` removes the
  swapfile, its fstab lines and the kernel arguments. The kernel arguments take
  effect on the next boot.

  The file is never resized in place: growing it would move it on the disk, and
  the kernel would then resume from the wrong place. Change the size by turning
  it off and on again.

- **`kuma doctor` grades it**, and grades the part that fails silently. If the
  swapfile and the kernel arguments disagree, a hibernated machine boots fresh
  and the session is gone with nothing logged. Doctor compares the two and says
  so. Running `kuma hibernate --yes` on a machine that already has a usable
  swapfile repairs exactly that, leaving the file where it is. Machines with no
  swapfile are not graded, because they promise nothing.

- **Secure Boot machines are told the truth.** A kernel that booted with Secure
  Boot on runs locked down, and a locked-down kernel refuses to hibernate. Kuma
  can still make the swapfile and set the kernel arguments correctly, and the
  machine still will not do it. `kuma install` and `kuma hibernate` say so
  before you spend the disk on it, and `kuma doctor` warns rather than reporting
  a machine ready that never was. If you want hibernate on such a machine, turn
  Secure Boot off in firmware; otherwise `kuma hibernate --off --yes` takes the
  space back.

- **The swapfile is labelled for SELinux.** `systemd-sleep` can only read a
  file typed `swapfile_t`, and the policy's own default for a file under `/var`
  is `var_t`, which it cannot read. A machine with the wrong label has a
  correct swapfile, correct kernel arguments and active swap, and fails at the
  moment you ask it to hibernate. Kuma images now declare that path a swapfile
  and relabel it at boot, and `kuma doctor` grades the label.

### Known limits

- **Hibernate does not work under Secure Boot**, and that is the kernel's
  decision rather than kuma's. See above: everything kuma sets up is correct and
  the kernel still refuses. Turning Secure Boot off in firmware is the only way
  to have both.
- **Tested in a virtual machine, not on a laptop lid.** CI installs a machine
  with a swapfile, hibernates it, confirms it powered off, boots it again, and
  checks the kernel's own `boot_id` to prove it resumed rather than started
  fresh. It then boots the same disk on firmware with Microsoft's keys enrolled
  to check that kuma reports the refusal rather than claiming readiness. What
  none of that covers is your hardware: lid-close behaviour, firmware that
  mishandles S4, and drivers that do not survive a suspend all vary by machine.

## v0.14.0 (2026-08-20)

A declaration describes a system; it never described your files. `[backup]`
copies them somewhere else, and `kuma install --restore` puts a machine back.

### Added

- **`[backup]`** copies what `[snapshots]` keeps to a restic repository, on a
  timer, reading from a snapshot so nothing changes mid-copy. Requires
  `[snapshots].enable`; `kuma check` says so rather than the unit failing at
  3am.

- **The credential is named, not held.** `secret = "backup"` points at
  `/var/lib/kuma/secrets/backup.env`, mode 0600, which you create. A
  declaration is committed and baked world-readable, so it is the wrong place
  for a password; a repository address containing one is refused. Recovering a
  machine therefore needs two things: this file and that credential.

- **`network_connections`** carries `/etc/NetworkManager/system-connections`,
  and is **off by default**. Those files hold a passphrase per network and
  nothing else can recreate them, so `kuma doctor` names which way it is set.

- **`kuma backup`**: bare reports without touching the network, `--init` seeds
  the first copy, `--list` asks the repository, `--restore` brings a path back
  after a dry run.

- **`kuma install --restore <file>`** rebuilds a machine from the repository.
  One file carries the address and its credentials. The restore runs at first
  boot, after `/var/home` becomes a subvolume; if the repository is
  unreachable that boot, the next one tries again.

- **`kuma doctor` grades backups** on a stamp only a run that copied something
  writes, so a machine that has quietly stopped is visible. Staleness follows
  your declared interval. It also grades the credential's mode.

### Changed

- Retention applies every copy; pruning runs weekly, because pruning repacks
  and moves far more data than forgetting a snapshot does.
- `kuma check` on a valid declaration now names the next command, and its JSON
  carries `actions` either way.
- `kuma init` no longer pins `system.base`, so a first declaration composes its
  own base like every published image.
- `doctor`'s dangling-enablement check reports as `enablement` rather than
  `units`, which the failed-unit check already used.
- `kuma switch` pipes the image into root storage instead of staging 1.5 GB
  through a temp file, and `doctor` runs its podman probes concurrently.

### Fixed

- **`kuma install --restore` left the repository credential world-readable**
  for the length of an install.
- `kuma install --json` emitted no JSON on failure and printed progress into
  the document.
- `backup.repo` reached generated shell without validation.
- `kuma install --groups` was unvalidated where a declaration's groups are.
- `kuma-brew-setup` wrote as root into a directory tree a normal account owns;
  it refuses a prefix it does not own.
- A live session no longer arms kuma's timers or converges Flatpak
  permissions.
- `[system.ca_certificates]`, added in 0.13, was documented nowhere.


## v0.13.0 (2026-08-20)

State that survives every rebuild, that the declaration could not express and
nothing on the machine would report. This closes the biggest of it and draws
the line around the rest.

### Added

- **`[overrides]`** declares Flatpak permissions, per app and per scope.
  Convergence is **per key, not per file**: kuma sets the keys you declare,
  removes the keys it set that you stopped declaring, and leaves every other
  line alone, so Flatseal stays usable and this file stays the record. The
  shape is Flatpak's own override file rather than `flatpak override`'s
  flags, and `flatpak override --show` round-trips into it. Applied at boot
  and by `kuma sync`, never on the daily timer, because a permission changing
  under a running app is indistinguishable from a bug.

- **`[system.ca_certificates]`** declares certificate authorities to trust,
  keyed by the name each gets on disk, with the certificate inline: a
  declaration pointing at a path elsewhere is not one file. A private key
  there is refused, since it would be baked world-readable into every image.

- `kuma doctor` reports a unit that is enabled with no unit file, and
  `kuma add --flatpak` refuses an id Flathub does not list.

### Changed

- `kuma sync` says which declaration it converged to, so a machine converging
  to the image's baked lists rather than the file in your hand says so.
- `kuma add`, `kuma remove` and `kuma capture` no longer claim flatpak and
  brew changes apply immediately when they do not.

### Fixed

- A declared `system.timezone` produced exactly one file and nothing graded
  it, because it arrives as a symlink rather than a copy or a redirect.


## v0.12.0 (2026-08-19)

Claims that were true and unchecked became commands that pass or fail, after a
converger stopped converging on the day 0.11.0 shipped and only a person
reading a journal could tell.

### Added

- **`kuma menu`**, bound to `Mod+D` on niri: applications, connect,
  declaration, system, notifications, power, drawn by the launcher kuma
  already ships. It lists applications itself rather than opening a second
  launcher, honouring the desktop entry spec (`NoDisplay`, `TryExec`,
  `Hidden`, `OnlyShowIn`, `NotShowIn`, `Terminal`, field codes, shadowing),
  and orders by launch count. Opening a group narrows what is shown, never
  what typing can reach.

  `kuma menu --list` prints the rows instead of drawing them, for ssh, a VM
  with no session, or working out why a row is missing.

  The build repaints the icons the menu names into `/usr/share/icons/kuma`,
  because Adwaita's symbolic icons hardcode a near-black fill that is
  invisible on kuma's launcher background.

- Suspend, reboot and power off are reachable from the menu, and
  `NetworkManager-tui` ships on niri as what it offers for network settings.
- `kuma doctor` reports an override pointing at nothing, and a machine that
  has stopped converging rather than only one whose last run failed.
- AppImages run without a declaration naming anything.

### Fixed

- Every example declaration this project has shipped is tested against the
  current schema, so an old file keeps working.
- Every command the docs tell you to run is checked against the real CLI, and
  every verb is named somewhere a person reads.
- A keybinding that spawns a kuma verb names one that exists.
- Publishing an image runs the checks that install and boot it.
- One app's broken download no longer fails Flatpak convergence forever.
- `kuma sync` recovers a converger that spent its start limit, and `kuma
  doctor` quotes what a failed one actually said.
- **The boot menu names the version it boots.** Entries had been naming the
  version that previously held the slot.


## v0.11.0 (2026-08-18)

The media is a download. v0.10.0 built the ISO in CI and booted it on every
push to prove it could, and left attaching it to a release switched off until
that job had a history rather than a first day; it has one, so a release now
carries the thing you write to a USB stick. The walkthrough leads with it,
because "describe a machine, build an image, then make your own media" was the
order a project with nothing to download had to teach.

One thing the release also fixes is the assertion that guards v0.10.0's
signature policy, which could not see the policy going missing.

### Added

- Releases carry the live ISO. A tag builds it, boots it, signs it with
  Sigstore like every other release asset, and attaches it to the release that
  already exists, so downloading kuma and installing kuma are the same page.
  Booting it is not a formality: the same script CI runs starts the ISO under
  UEFI and asks the live session whether it reached a desktop, so the file on
  the release page is one that came up rather than one that built. This was
  wired in v0.10.0 and left off, waiting on the job that builds it having a
  run history rather than on a tag being its first real exercise; ci.yml has
  built and booted the ISO on every push to main and on a daily cron since,
  and went green before this was turned on.

### Fixed

- The install-and-boot smoke tests could not see a missing signature policy.
  They asserted that `kuma doctor` reports nothing graded `fail`, and the three
  ways this control goes missing are all graded `warn`: no policy file, one
  that will not parse, or one that does not name kuma's repository. Only a
  policy naming a key it does not have, or one with nowhere to look for
  signatures, was ever `fail`. So the scan saw the half-broken states and was
  blind to the absent one, which is the likeliest of the three and the one an
  `/etc` merge can cause. An installed machine now has to grade `signatures`
  as `ok`, which is the requirement rather than "not fail" because every image
  writes the policy, the key and the registries.d entry unconditionally. The
  cross-version job reports whether upgrading brings the policy to a machine
  installed before it existed, and fails only if an upgrade takes it away.

### Changed

- The getting-started walkthrough leads with installing a machine rather than
  building an image. It was ordered "build an image, then build media" because
  media was something you had to make yourself, and it said so; with media on
  the release page the front door is download, boot, install, and describing
  your own machine is what you do next rather than what you do first. The
  builder's path is unchanged and still there, one step later.

## v0.10.0 (2026-08-17)

The release that makes kuma installable by somebody who is not its author: a
download link becomes a booted machine, with no clone and no toolchain.

### Added

- **CI builds the live ISO, boots it, and keeps it as an artifact**, with a
  size guard below GitHub's 2 GB asset cap. Until now the media a stranger
  downloads was built by hand on one laptop.
- **Every image refuses an unsigned kuma update.** Images carry kuma's public
  key and a `policy.json` naming it, and `kuma doctor` grades that the machine
  actually requires a signature.
- **`kuma doctor --report`** prints what to attach to a bug report: findings,
  version, booted digest, and the declaration with secrets redacted.
- Fedora 45 bases are named Callisto.

### Changed

- A machine says which kuma built it: `PRETTY_NAME` is `Kuma <version>
  (<bear>)`, rewritten even when no bear matches the base.
- `update` and `update --check` report `fedora_release` in one shape, and
  `kuma update` says when it is about to change your Fedora release.
- The live ISO's boot menu carries a serial console.
- Both Font Awesome generations are installed and listed in waybar's font
  stack.
- `SECURITY.md` names the two package sources a desktop brings in beyond
  Fedora's own, and the README says kuma has only been booted on AMD
  graphics.
- The walkthrough describes installing from published media rather than from
  a clone.


## v0.9.0 (2026-08-16)

CI boots and installs what it builds, so a release no longer depends on
somebody booting it by hand.

### Added

- **The boot and install stages run in CI**, on every committed example.
  `scripts/smoke.sh --published <image>` installs an image kuma published and
  boots the disk it wrote; `--upgrade-to <new>` installs an older release and
  moves it forward; `--encrypted` installs a LUKS disk and unlocks it at the
  console.
- The boot checks ask the machine to grade itself with `kuma doctor --json`,
  and no unit named `kuma-*` may be failed.
- The disk under test gets `console=ttyS0`, so a machine that never boots
  still leaves evidence.

### Fixed

- **`kuma-home-subvol` and `firewalld` no longer race for `/var/home`.** The
  converger runs in an early slot instead of ordering itself against a list of
  units, and it now says why it declined rather than exiting silently.
- `kuma install` no longer refuses a disk for want of a tool that is present.
- The bar showed bluetooth twice, and two session services had two launch
  paths each.


## v0.8.1 (2026-08-16)

The snapshot timer takes a snapshot.

### Fixed

- The snapshot script asks `findmnt` which filesystem holds the target
  rather than what is mounted exactly at it. A btrfs subvolume does not
  have to be a mount point, and on a machine kuma installs `/var/home` is
  one nested inside the deployment's `/var`: the bare form printed
  nothing, so the script decided the target was not btrfs and exited 0
  having taken nothing, while `kuma doctor`, which has always asked with
  `-T`, said the target was fine. This was the second half of the same
  bug as the missing subvolume, and it survived fixing the first: an
  install from the v0.8.0 image gets a proper subvolume and still took no
  snapshot until this.

## v0.8.0 (2026-08-15)

Disk encryption, and the install path a stranger takes.

### Added

- **`kuma install --encrypt`** makes the root a LUKS volume. The passphrase is
  asked for on a terminal, read from stdin, and never appears in a flag, a
  file, or the process list. Nothing keeps a copy: a lost passphrase is a lost
  disk.
- `kuma doctor` says whether the root is encrypted.
- `kuma install` says when the image it is installing declares an account of
  its own, and defaults to the image its installer media was built from.
- Bare `kuma` says `converging` while a sync unit is running, rather than
  reporting drift against a machine that is mid-convergence.
- `scripts/smoke.sh --install` writes a real encrypted disk and verifies it.

### Fixed

- **Every image gives `/var/home` a btrfs subvolume on first boot**, without
  which `[snapshots]` silently took nothing on machines kuma installed.
- `kuma doctor` grades `kuma-user-sync` on installed machines, where the
  account is created rather than declared.
- `kuma clean` reclaims what `kuma iso --live` leaves behind.


## v0.7.0 (2026-08-15)

Installing became something a live session can actually do.

### Added

- `[system].shell` declares the login shell accounts get, separately from
  `[user].shell`, so shareable media can carry it.
- `kuma install` partitions the disk itself, takes a file as a target for
  building disk images, refuses a disk under 16G before asking anything, and
  refuses a `localhost/` image as an update source. `--update-from` installs
  one image while tracking another.
- A live session offers `kuma install` as its one affordance.
- The composed base ships `ncurses`.


## v0.6.0 (2026-08-14)

`kuma install`, and the media to run it from.

### Added

- **`kuma install`** installs kuma onto a disk. With no `--disk` it lists what
  it found and asks; `--image` defaults to the published image. Whole-disk and
  destructive: `bootc install to-disk` owns the layout.

  It asks for an account and a hostname, since a published image can declare
  neither, and writes the answers to `/var/lib/kuma/user`, which bootc fills
  from the image once at install and never touches again. Without `--yes` it
  describes what it will ask for rather than asking.

- **`kuma iso --live`** builds installer media in which the image is its own
  live root. Media for trying kuma, not yet for installing from. The live
  session runs SELinux permissive.

- The composed base ships firmware for Intel wifi and SOF audio.


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
