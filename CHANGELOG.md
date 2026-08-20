# Changelog

## Unreleased

Entries land with the change they describe; the next tag takes this section
as its release notes.

### Added

- `[overrides]` declares Flatpak permissions, per app and per scope. The
  declaration could always name an app; it could not say what that app is
  allowed to touch, so every permission on the machine was undeclared state
  that survived every rebuild and that nothing could see.

  **Convergence is per key, not per file.** kuma sets the keys you declare,
  removes the keys it set that you stopped declaring, and copies every other
  line through untouched, so Flatseal stays the editor people reach for and
  this file stays the record. The two meet at `kuma capture`. A key kuma never
  set is never kuma's to delete, even when it sits in the same group and
  contradicts what was just declared.

  The shape is Flatpak's own override file rather than `flatpak override`'s
  flag strings, which is a consequence of the above and not a preference: from
  a flag string kuma cannot tell which file key a `--nofilesystem` lands in
  without reimplementing flatpak's parser, and not knowing the key means the
  only safe convergence is replacing the whole file, which is exactly the
  authority per-key ownership refuses to take. `flatpak override --show`
  round-trips into it.

  Both stores are covered. System scope is the default because that is where
  kuma installs the apps it declares; `scope = "user"` writes the per-user
  store, applied by a `systemd --user` unit rather than by root reaching into
  a home. One app declares into one store.

  **`kuma diff` and `kuma capture` see them**, which is what makes the
  declaration owning permissions mean anything. diff reports both directions,
  a declared key the machine lacks and a key kuma set that the declaration
  stopped naming, and stays silent about keys kuma never set: those are
  somebody's machine, not drift. capture offers the reverse, turning a
  permission you toggled in Flatseal into a declaration entry, and offers it
  only for apps the declaration already installs, because a machine
  accumulates override files for software that left years ago. `kuma check`
  counts them.

  **Applied at boot and by `kuma sync`, deliberately never on the daily
  timer.** An install arriving at a random hour is additive and idempotent; a
  permission reverting at a random hour changes what a running app can reach,
  and a toggle that silently flips back tomorrow afternoon is
  indistinguishable from a bug. The rule fits in a sentence: declared
  permissions are restored when you boot, and the session in between is yours.

- `kuma doctor` reports a unit that is enabled and has no unit file. Found by
  reading a real machine: an enablement symlink in `default.target.wants`
  pointed at a unit file that had been deleted, `systemctl is-enabled`
  answered `not-found`, and every other surface said the machine was in sync.
  Nothing fails, because a unit that was never found can never fail, which is
  exactly why nothing reported it.

  Narrow on purpose. `/etc` on a real machine carries dozens of legitimately
  local files and a check that lists them all is one people learn to scroll
  past, but a link into a `.wants` directory is a machine saying it will start
  something at every boot, and a missing target makes that sentence false.

- `kuma add --flatpak` refuses an id Flathub does not list. The converger
  installs the whole declared list in one command, so a name that does not
  resolve fails the unit and the apps beside it never install, on that boot
  and every boot after: a typo taking down convergence for everything else.
  `[services]` has checked unit names at build time since it existed and this
  list checked nothing at all.

  Checked against flatpak's own cached appstream data, never the network, so
  declaring a package cannot hang on a captive portal or fail on a plane. No
  cache to read means the check does not run rather than that the name is
  wrong, and a refusal names the way out, since a cache can be behind a
  genuinely new app.

- `[system.ca_certificates]` declares the certificate authorities a machine
  trusts on top of the ones Fedora ships, keyed by the name each one gets on
  disk. The certificate goes in the declaration rather than beside it: a file
  that points at a path somewhere else is not one file any more, and a CA
  certificate is public by construction, which is what makes that safe here
  and unsafe for anything in `[user]`.

  A private key pasted in by mistake is refused rather than warned about,
  because it would be baked world-readable into every image built from that
  declaration and pushed to a registry. The anchors land under /etc by a COPY,
  so `kuma doctor` watches them for free, and `update-ca-trust` runs in the
  layer that adds them rather than leaving a trust store that only becomes
  true at boot.

### Changed

- `kuma sync` says which declaration it converged to. It starts the same
  convergers boot does, those read what the image baked, and an edit that has
  not been built cannot reach them however often sync runs. It reported
  "Converged" anyway and then offered `kuma diff` to "confirm the machine now
  matches its declaration", which was an instruction to go watch it fail.

- `kuma add`, `kuma remove` and `kuma capture` no longer end with "flatpak and
  brew changes converge on the machine at boot and daily". That sentence sat
  under the `kuma build` and `kuma switch` edges and read as the alternative to
  them, when what converges at boot and daily is the previous list, forever,
  until a build of the edit boots. They now say that instead.

### Fixed

- A declared `system.timezone` was owned by the image and watched by nobody.
  kuma writes the timezone as `ln -sfn /usr/share/zoneinfo/<zone>
  /etc/localtime`, and the scan that works out which /etc files an image owns
  read only COPY destinations and shell redirects, so the single file that key
  exists to produce fell outside every check kuma has.

  It matters most on an installed machine, because Anaconda writes its own
  `/etc/localtime` first, and ostree's merge keeps a local file over every
  future image: a declared timezone would simply never take effect, with
  nothing anywhere saying so. `kuma doctor` now grades it like any other file
  the image owns, and `ln -s` is read as the third way a build writes into
  /etc.

## v0.12.0 (2026-08-19)

This release is about the difference between a thing being true and a thing
being checked. It exists because the day v0.11.0 shipped, a machine stopped
converging and only a person reading a journal could tell.

Three of its claims were true and unchecked: that a converger recovers on its
own, that the walkthrough a stranger types still describes the tool, and that
a declaration written for any released kuma keeps working. All three are now
commands that pass or fail.

It also grew a face. `kuma menu` was meant for a later release and was built
on a branch to keep this one honest; it earned its way in by being finished,
and by being the first thing here a person sees rather than a thing they can
check. The rule that governs it is the same one this release is about: a menu
entry is one keystroke with no diff and no pause, so it may change machine
state immediately and may never write your declaration.

### Added

- `kuma menu` opens a menu in the desktop's own launcher: apps, connect,
  declaration, system, notifications, power. Bound to `Mod+D` on niri, which is
  the key that used to open the bare launcher; the menu lists applications
  itself, so two keys for one job would have left one of them showing strictly
  less. The stock bind is grepped for before it is replaced, so a niri release
  that renames it fails the build rather than shipping media whose main key
  does nothing.

  The desktop kuma assembles had no face of its own, so every device-level
  setting was somebody else's control panel and nothing at all owned the
  system. This is
  not a settings application, which is a desktop environment's job and never
  finishes; it is a tree rendered by `fuzzel --dmenu`, themed by the fuzzel
  config kuma already ships, in the launcher the clipboard picker already
  uses. Nothing new is installed and nothing new is drawn.

  **Applications are rows in it.** Apps was a group holding one entry that
  opened a second launcher on top of the first; now the menu lists the
  applications itself, so typing `firefox` at the top finds Firefox the same
  way `reboot` finds reboot. That means implementing the desktop entry spec
  rather than gesturing at it, and the parts people skip are the parts that
  bite: on the machine this was written on, 11 of 32 entries are
  `NoDisplay=true` and must never appear, and 12 carry field codes (`%U`,
  `%F`) that arrive as literal arguments if nothing strips them. `TryExec`,
  `Hidden`, `OnlyShowIn`, `NotShowIn`, `Terminal` and the spec's quoting are
  all honoured, entries earlier in the search path shadow later ones, and
  launch counts are kept so the list is not alphabetical forever.

  **`kuma menu --list` prints the rows instead of drawing them**, marking
  which are on screen at rest and which only typing reaches. How to see the
  menu over ssh, in a VM with no session, or when wondering why a row is
  missing. `scripts/smoke.sh` runs it on every installed machine it boots,
  which is the only automated thing that touches the menu at all.

  **Descending narrows what is shown, never what can be found.** Opening a
  group shows that group's rows and a way back; every other row is still in
  the list, so someone who opens Connect and then remembers they wanted to
  reboot types `reboot` and gets it, rather than finding that the menu quietly
  became a smaller menu.

  **It opens on its groups and searches every row.** A launcher can only match
  against the lines it was handed, so a menu of submenus cannot be searched:
  typing `reboot` at the top of one matches nothing and the person who knew
  what they wanted navigates anyway. Every row is in one list, the groups come
  first, and the window is sized to exactly their number, so browsing sees six
  sections while typing reaches all of them. `reboot` finds the row, `power`
  finds the group and its five. A group's row descends into just its own.

  **The build gives the menu its own icon theme.** Adwaita's symbolic icons
  are the right drawings and the wrong colour: each hardcodes a near-black
  fill, fuzzel renders the file as it is, and kuma's launcher is `#0e1626`, so
  they drew invisibly. Every icon the menu names is now copied and repainted
  in the launcher's own foreground into `/usr/share/icons/kuma`, which also
  buys alignment that text glyphs cannot: fuzzel's icon column is a
  fixed-width slot, while a proportional face gives every glyph a different
  width and leaves the labels ragged.

  Any hex is rewritten rather than one known value, because the twenty files
  carry three different darks and a sed for the common one would have left two
  icons invisible, which reads as a glitch rather than as a bug.
  `fill="none"` is left alone, since it means transparent. Then the step
  checks its own work and fails the build naming the file, because a generator
  that cannot say whether it worked is how the icons went out invisible the
  first time. The colour is asserted equal to `assets/fuzzel.ini`'s, so the
  icons and the text they sit beside cannot part company.

  A row that prints keeps its window. A terminal launched as `kitty -e
  <command>` closes the moment the command exits, so Health would have shown a
  sudo prompt and vanished as the password was finished, and every row that
  runs for a second is exactly the one whose output is never read. The command
  runs through a shell that waits, which also reports a non-zero exit the
  closing window would otherwise have swallowed.

  Selection comes back as an index rather than as text, and fuzzel is asked
  for `--only-match`, so a typo that matches nothing cannot be mistaken for a
  choice. The window is sized to its longest row rather than to the app
  launcher's width, and a row inside its own group drops the prefix that the
  group heading already said.

  **The menu never writes the declaration.** `kuma capture` is the one
  deliberate path from what the machine has to what the file says, and its
  safety is the ceremony: dry run, review, confirm. A menu entry is one
  keystroke with no diff and no pause, so a second writer would not add
  convenience, it would remove the only thing that made the first one safe.
  Declaration entries open the file in your editor or run a verb that asks for
  itself. Machine state (lock, suspend, notification mode) changes
  immediately, because that is the half a launcher is better at than a panel.

  Entries appear only when their program is present, so the menu offers the
  terminal tool where there is one, the graphical tool where there is not, and
  no row at all where there is neither. A group whose every entry is missing is
  absent rather than empty. Nothing runs as root to draw the menu, so opening
  it never prompts.
- Suspend, reboot and power off have a menu. Stock niri binds a lock and a
  quit and nothing else, so on a laptop the only way to suspend from the
  desktop was a terminal.
- `NetworkManager-tui` on the niri desktop. It is what the menu offers for
  the network in preference to the graphical editor, because a terminal
  program inherits the terminal's theme rather than arriving as a window from
  another system, and it is the only network tool left on a machine whose
  session will not start.

- `kuma doctor` reports a Flatpak permission override that points at nothing.
  A machine that was another distribution first can carry an override symlink
  into a directory that distribution shipped and kuma does not have, and it
  survives in `/var` across every image switch because nothing has ever looked
  at it. Graded a warning: flatpak tolerates it and the machine is not broken,
  but it is a statement about an app's permissions that is not true, and the
  declaration cannot see it yet. Regular override files are left alone; they
  are somebody's settings, whoever wrote them.
- `kuma doctor` reports a machine that has stopped converging. It already
  graded whether the last convergence attempt failed; it could not see a
  machine whose last attempt succeeded three weeks ago, because "last run
  succeeded" and "timer active" are both true of a machine that quietly
  stopped converging. Asked only of the convergers a timer runs again, which
  is what makes the age meaningful: they run at boot and on a daily timer that
  catches up after a machine was asleep, so seven days without one is seven
  missed firings and every boot in between. The account converger runs at boot
  only, so its age would measure uptime and it is not asked.
- Every declaration this project has shipped as an example is now tested
  against the kuma being built. Schema v1 is claimed permanent, and nothing
  held anyone to it: a renamed field or a changed default would have been
  found by somebody upgrading a machine whose declaration predated it. The
  corpus is every distinct example shape from every release, and an example
  that changes has to be recorded there before it ships. This covers old
  declarations on new kuma; the other direction, a newer declaration read by
  an older kuma, is still a hard error and still undecided.
- Every command the documentation tells you to run is now checked against the
  command line kuma actually has. Using the machine proves the tool works and
  never reads the docs, so a renamed flag is noticed the moment somebody types
  it while the page still naming the old one rots unread. This catches the
  words, not the behaviour: what the commands do is proven by CI where CI can
  reach it, and the walkthrough now carries a record of which of its commands
  that is true of.
- AppImages run on a kuma machine without a declaration naming anything. An
  AppImage is a squashfs its runtime mounts over FUSE before any of its own
  code runs, and Fedora ships only FUSE 3, so a downloaded AppImage failed at
  `dlopen(): error loading libfuse.so.2` on a machine that was otherwise
  complete. Both halves of FUSE 2 are in every image now: the library the
  runtime loads, and the setuid helper that library mounts with. They are
  separate packages and neither requires the other, so naming one looks like
  the fix and leaves a different error in the same place. Not gated on a
  desktop, because the packages are under a megabyte together and the failure
  is one somebody meets by opening a file they downloaded.

### Fixed

- Every verb is named somewhere a person reads. One test walked the docs and
  checked each command against the CLI; nothing walked it the other way, which
  is the direction a new verb goes missing in. `kuma menu` was written, tested,
  and documented nowhere while every test passed. The exception is named rather
  than filtered by a rule: `kuma boot-titles` is hidden because a systemd unit
  runs it and nothing asks a person to.
- A keybinding that spawns a kuma verb now names one that exists. The binds
  are strings in a file the compiler never reads, so a renamed verb left a key
  that did nothing and said nothing; a test walks every `spawn "kuma"` in the
  shipped binds and checks the verb against the CLI's own definition.

- Publishing an image now runs the checks that install and boot it. They read
  the moving tag a publish moves, so running them beforehand only re-tested
  the previous release, and keeping the order right was something a person had
  to remember. They run after the publish, so nothing they find can unpublish
  anything; what they can do is say so the same day.
- One app's broken download no longer fails Flatpak convergence forever. A
  remote can serve a static delta whose decompressed part is larger than
  ostree will accept, and the limit is computed per machine, so the same
  update fails byte-identically on every retry: Flathub's Firefox did this,
  the unit failed six times, systemd stopped trying, and the machine sat
  unconverged until someone read the journal and ran flatpak by hand. Both
  download paths now retry without static deltas, which trades the delta's
  bandwidth saving for a whole download on the path that already failed and
  changes nothing anywhere else. The retry covers the declared-install pass as
  well as the update, because `--or-update` means the install is where an app
  already present takes its new version, and that is the pass that failed.
- `kuma sync` recovers a converger that spent its start limit. The units are
  configured to retry a handful of times before systemd gives up, and a unit
  in that state refuses `systemctl start` outright. `kuma doctor` prints
  `kuma sync` as the fix for a failed converger, so the prescribed fix was
  refused by systemd rather than run, and the only way out was knowing to run
  `systemctl reset-failed` first. Sync now resets before it starts.
- `kuma doctor` quotes what a failed converger actually said. `last run:
  exit-code` is true and says nothing a person can act on; the sentence naming
  the cause was one `journalctl` away, for anyone who knew to look. The line
  now carries it, read from the failed run's own output rather than filtered
  out of the unit's journal by string, so systemd's own "Failed to start" is
  never mistaken for the service's explanation of why.
- The boot menu names the version it boots. ostree rewrites a boot entry only
  when the kernel or the kernel arguments move, and a kuma release moves
  neither: the lock pins the base digest, so a rebuild reuses the same
  composed base and the same kernel. Every deploy therefore rotated the
  deployments underneath entries whose titles stayed where they were, and each
  entry ended up naming the version that used to hold its slot. A machine
  booted into 0.12.0 offered `Kuma 0.11.0` as its default and `Kuma 0.10.0` as
  its rollback. The order was always right, so the menu booted what it should;
  it named all of it wrong, and it does that at the one moment the menu is
  what somebody is reading, which is when the machine will not come up far
  enough to run `kuma rollback`. Each entry now takes its title from the
  deployment its own `ostree=` argument points at, rewritten at boot and again
  after the deployments rotate at shutdown, and `kuma doctor` grades the
  result rather than only naming the problem.

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

The artifact a stranger downloads is built and booted by CI rather than by
hand on one laptop, an update that did not come from this project is refused
rather than installed, and a machine that would not boot has something to
paste.

Fedora 45 also moved two things underneath this release, and both were found
by building against it rather than by reading its release notes.

### Added

- CI builds the live ISO, boots it, and keeps it as an artifact. The ISO is
  the one thing a stranger downloads and was the one thing built by hand on a
  single laptop, so its failures were found by whoever tried it next.
  `scripts/smoke.sh --iso` builds it, refuses one too big to ride a GitHub
  release asset (1.77 GB against a 2 GB cap, and every desktop package spends
  the difference), boots it under UEFI, and asks the live session whether it
  is actually running: systemd healthy, no failed units, a graphical session
  on seat0, greetd and niri both up. It asks over the serial console because
  installer media has no disk to inspect and its account has no password for
  ssh to use. Attaching the ISO to a release is wired but off by default,
  waiting on this job having a run history rather than on a tag being the
  first real test. When it is turned on the ISO is signed like every other
  release asset and the build runs after the release exists, so a live
  session that fails to come up cannot take the binary's release with it.
- Every image now refuses an unsigned kuma update. Images carry kuma's
  signing key at `/etc/pki/containers/kuma.pub` and a
  `/etc/containers/policy.json` requiring a valid signature for
  `ghcr.io/letdown2491/kuma`, plus the `registries.d` entry telling
  containers/image where cosign keeps signatures. Published images have been
  signed since v0.6.0 and SECURITY.md explained that a key pair was chosen
  precisely so a policy could name the key, but no policy was ever written,
  so nothing on any machine checked. `kuma doctor` grades it, because a
  signature nobody verifies is a claim rather than a control. The rule covers
  kuma's own repository and nothing else: that file is shared by podman and
  bootc, so a blanket requirement would refuse Fedora's base on the next
  update and your own local build on the next switch.
- `kuma doctor --report` prints what to attach to a bug report: the findings
  `--json` already carries, plus which kuma is running, which image is booted
  and its digest, and the declaration the machine was built from. Those last
  three are the questions always asked first and the ones `--json` did not
  answer, which left a stranger whose machine did not boot with nothing
  useful to paste. `user.password_hash` is removed, by parsing rather than by
  rewriting lines, because the value can be quoted four ways and a report is
  pasted by somebody who will not read it first. The same goes for anything
  else on a short list of secret-bearing key names, wherever in the file it
  sits, and a secret that survives the redaction costs the report its
  declaration rather than getting published. A declaration kuma cannot parse
  is omitted rather than pasted raw: not being able to redact a file is not a
  reason to publish it.
- Fedora 45 bases are named Callisto, after the nymph Zeus placed in the sky
  as Ursa Major. Bear names follow the alphabet where a letter has a name
  worth using.

### Changed

- A machine now says which kuma built it: `PRETTY_NAME` is
  `Kuma <version> (<bear>)` rather than `Kuma (<bear>)`, and `VERSION`
  matches it. The old wording dropped the number on the grounds that kuma had
  no version of its own, which stopped being true once releases were tagged.
  `VERSION_ID` is untouched and stays Fedora's, because toolbox, distrobox
  and COPR resolve against it.
- `VERSION` is rewritten even when no bear matches the base. Left alone it
  kept Fedora's own string, so an image built on a branched base announced
  itself as `45 (Rawhide Prerelease)` while calling itself Kuma everywhere
  else.
- `update` and `update --check` both report `fedora_release` as one shape:
  `current`, `changed`, `from`, `to`. A caller reads one key rather than
  type-checking it.
- `kuma update` says when it is about to change your Fedora release, and
  `kuma update --check` says which release you are on. A Fedora major showed
  up in the lock diff as several hundred package lines and nothing that named
  the release, so the largest change kuma can make to a machine was the one
  it described least. The line prints after the diff and before the staging
  gate, so it is visible while nothing is staged and `--yes` is still
  required. Both numbers are read out of the images themselves rather than
  from their tags, because `fedora-bootc:45` is not a promise that what is
  inside came from Fedora 45: a branched release carries rawhide's repo
  definitions for a while, and composing "from 45" can produce a base that
  calls itself 46.
- Both Font Awesome faces now arrive through `fontawesome-fonts-all` instead
  of being named per face. The per-face package names carry the major
  version, which changed under us (`fontawesome-6-*` became `fontawesome-7-*`
  in Fedora 45) and would have failed the build. The metapackage's name does
  not, it pulls exactly the same two packages, and it owns no files. Font
  Awesome 7 is also about a megabyte smaller installed than 6.
- waybar's stylesheet lists both Font Awesome generations in its font stack.
  Family names carry the major version too and that one is baked into the
  font's own metadata, so no package choice avoids it. Getting it wrong does
  not fail a build; it silently drops every icon in the bar to the fallback
  sans.
- The live ISO's boot menu carries a serial console, and no longer prints an
  error before it. `load_video` is a function Fedora's own grub.cfg defines
  rather than a grub builtin, so calling it produced `can't find command
  'load_video'` on every boot of the installer media, which is the first
  thing a person sees when they try kuma; `insmod all_video` already does
  that work. `console=ttyS0,115200 console=tty0` on both entries makes a live
  boot readable in a VM, which is where `kuma iso` tells people to try it,
  and tty0 comes last so the screen stays the primary console on real
  hardware.
- SECURITY.md names the two package sources a desktop brings in beyond
  Fedora's own. The trust-boundary section said kuma "adds no third-party
  repositories", which was true of what a declaration can ask for and false
  of what kuma itself does: every desktop build adds RPM Fusion for the
  freeworld video codecs, and `fedora-cisco-openh264` reaches the image as a
  weak dependency because Fedora enables that repo by default. A reader
  auditing that section would have concluded neither was on their machine.
  Both are the same call Fedora Workstation makes, and a declaration with no
  desktop still reaches neither, which is now a test rather than a claim.
- The README says kuma has only ever been booted on AMD graphics. Images
  carry Intel and NVIDIA firmware, i915, xe and nouveau, and Intel's Mesa and
  Vulkan drivers, and CI boots every build on a virtio GPU, but none of that
  is a report from somebody whose laptop has Intel graphics in it. A test now
  pins the Intel and Broadcom firmware by name rather than by iterating the
  list it is checking, which is how the previous version of this passed while
  every Intel laptop booted with no wireless and no sound.
- The walkthrough says what the machine now does, and its example declaration
  is checked by a test rather than by whoever reads it next. It also states
  plainly that there is no ISO to download yet: CI builds one on every push
  and boots it, and nothing publishes it, so media is still something you
  build.

## v0.9.0 (2026-08-16)

CI boots and installs what it builds, so a release no longer rests on
somebody having booted it by hand.

The first thing those checks found was a race that had been shipping for
several releases, on a fraction of first boots, in two directions that
looked like unrelated bugs.

### Added

- The boot and install stages run in CI. They were local-only on the
  stated grounds that they need KVM and sudo, which was measured rather
  than argued and turned out to be one udev rule: `/dev/kvm` is present on
  a hosted runner and the CPU exposes `svm`, and the runner user simply is
  not in the `kvm` group. What genuinely does not work there is
  `-display egl-headless`, which needs a DRM render node on the host and a
  runner has no GPU; plain `virtio-vga` still gives the guest a virtio-gpu
  device, and a niri image brings up its greeter through it on llvmpipe
  alone. `QEMU_VGA` and `QEMU_DISPLAY` select both.
- `scripts/smoke.sh --published <image>` installs an image kuma published
  and boots the disk that install wrote. It is the only check that reads
  the registry rather than the tree, so it lives in its own workflow: a
  registry hiccup reddening somebody's pull request teaches people to
  ignore red. It also reaches a branch nothing else could. Every other
  disk here comes from bootc-image-builder with an ext4 root, and only
  `kuma install` writes btrfs, so until now the assertion that `/var/home`
  is its own subvolume had never once run in anger, and
  `kuma-home-subvol` is what stands between `[snapshots]` and a timer that
  takes nothing.
- `scripts/smoke.sh --published <old> --upgrade-to <new>` installs an
  older published version, points its update origin at a newer one,
  upgrades with `bootc`, reboots, and asks whether the machine survived.
  Nothing had ever checked that a machine installed at one version can
  reach a later one, which is the promise everything else rests on. It
  passes: a 0.7.0 machine reaches the current image, boots, stays healthy
  and keeps its account. It also reports what does not travel, and
  `/var/home` is the worked example.
- `scripts/smoke.sh --published <image> --encrypted` installs an encrypted
  disk, types the passphrase at the guest's serial console, and boots it.
  Encrypted installs were already verified thoroughly and entirely
  offline, through a loop device: the LUKS header, the passphrase, the
  kargs and the account file. What that could not answer is the only
  question a person has, which is whether the machine comes up when you
  type the passphrase, so the feature that headlined v0.8.0 was the one
  shipped thing with no boot coverage at all. It comes up. The serial
  console is a socket for every run rather than a file, because two
  console paths would mean the encrypted one is the only one nobody
  exercises.
- The boot checks ask the machine to grade itself. `kuma doctor --json`
  reporting nothing failing is now an assertion, so every check added to
  doctor becomes a boot assertion with no change to the harness.
- No unit whose name starts with `kuma-` may be in a failed state.
  `systemctl is-system-running` reports a unit that died and a unit that
  declined identically as `degraded`, and the boot stage accepts
  `degraded` as settled, so a dead converger had nowhere to show up.
- The installed disk under test gets `console=ttyS0` before it is booted.
  The image sets no console karg, correctly, but the consequence was that
  the worst failure produced the least evidence: a machine that never
  booted wrote firmware output and then nothing, which is what a UEFI
  mismatch looked like for seven silent minutes.

### Fixed

- `kuma install` no longer refuses a disk for want of a tool that is
  installed. The preflight searched four fixed directories and not
  `/usr/local`, which is where kuma's own README tells you to put the kuma
  binary, so a machine keeping `podman` there was told to install podman.
  The deeper half is that the check and the install script were two
  questions that had to agree by luck: the script runs under `sudo` and
  resolved commands through `secure_path`, which is configured per machine
  and need not include `/usr/local` either. Both now derive from one list.
- The bar showed bluetooth twice, in two styles. One was waybar's own
  module, a font glyph like every other module; the other was
  `blueman-tray`'s coloured application icon in the tray. A tray renders
  whatever icon it is handed and cannot recolour it, so the fix is one
  indicator rather than two that match: `blueman-applet` keeps running,
  because it is the agent that answers pairing requests, and only its
  status icon goes. Disabling `StatusIcon` alone does nothing, because
  `ShowConnected` depends on it and the plugin manager loads a dependency
  whether or not it was disabled.
- `kuma-home-subvol` and `firewalld` no longer race for `/var/home`, and
  they were one bug rather than the two they looked like. Making
  `/var/home` a subvolume means `rmdir` and then `btrfs subvolume
  create`, and between those two commands the directory does not exist.
  Twenty five units on a desktop image carry `ProtectHome`, which has
  systemd mount something over `/var/home` before the service starts, so
  the window had two losers. When the converger won, firewalld started
  into a missing directory and died with `226/NAMESPACE`. When a
  sandboxed unit won, its mount pinned the directory, `rmdir` failed with
  `EBUSY`, the converger died, and `/var/home` stayed an ordinary
  directory for the life of the machine, which costs `[snapshots]`
  everything and reports nothing beyond `kuma doctor`. The converger now
  runs in the slot after `systemd-tmpfiles-setup` and before
  `sysinit.target`, which closes the window against all of them at once,
  including the three that start too early for any `multi-user.target`
  ordering to have reached. Ordering it against individual writers was
  the first attempt and the wrong shape: those units do not write to
  `/var/home`, systemd binds it for them.
- `kuma-home-subvol` says why it declined, and asks `findmnt` for one
  line. Declining is the right answer on every boot after the first, and
  it was a silent `exit 0`, so a boot where the converger should have
  acted and did not looked exactly like a boot where it correctly did
  nothing: the unit succeeded either way and the only difference was an
  inode nobody reads. The one-line answer matters because `findmnt`
  prints a line per mount when anything is stacked at or under the path,
  and a two-line answer never equals `btrfs`.
- Two session services had two launch paths each. `xdg-desktop-autostart`
  is active in the niri session, so Fedora's autostart entries for blueman
  and the mate polkit agent became units while `niri-extras.kdl` also
  spawned both. They are single instance, so one launch won and the other
  quietly lost, and the winner was not consistent between them on the same
  boot. kuma's spawn is the one kept, because relying on the autostart
  path means a session that never reaches it has no polkit agent, which is
  invisible until somebody needs an authentication prompt.

### Changed

- COSMIC is described as experimental. It builds on every push and it does
  boot; what it does not get is the install-and-interrogate verification
  niri now gets on every change.

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

The disk kuma installs can be encrypted, and the machine it makes says what
it is and what it came from.

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

- Every image gives `/var/home` a btrfs subvolume of its own on the first
  boot, without which `[snapshots]` could never take one. A snapshot is of
  a subvolume, `/var/home` on a machine kuma installed was an ordinary
  directory inside the deployment's `/var`, and the snapshot script exits
  0 on a target it cannot snapshot: the unit succeeded, the timer stayed
  active, the store stayed empty, and every report read healthy while
  nothing was ever taken. Machines installed by Anaconda were never
  affected, which is how it survived. The new `kuma-home-subvol` runs
  before any account exists and only while the directory is still empty,
  carrying the mode and SELinux label across, since a fresh subvolume is
  created unlabeled. `kuma doctor` now grades the target itself, for the
  machines already in that state, where the layout cannot be changed
  without moving home directories.
- `kuma install` on installer media defaults to the image that media was
  built from, when that image can be pulled. Installing fetches from a
  registry rather than copying the media, and the default was kuma's
  published image regardless: somebody could build media from their own
  declaration, boot it, look at their own desktop, install it, and get a
  different system with nothing saying so. Media built from a `localhost/`
  image, which is what `kuma build` produces, still installs the published
  one, because a local tag cannot be pulled from anywhere, and now says
  that out loud instead of leaving it to be discovered.
- `kuma doctor` grades `kuma-user-sync` on a machine that was installed,
  not only on one built from a declaration that names an account. A
  published image declares none by design, so `kuma install` writes the
  account to `/var/lib/kuma/user` on the target and the converger creates
  it at first boot; doctor only ever looked for the baked
  `/usr/lib/kuma/user`, so on every machine the install path produces, the
  check silently did not exist. That is where it is worth the most: a
  machine whose account was never created is one nobody can log in to, and
  doctor would have called it healthy.
- `scripts/smoke.sh` excuses a failed `systemd-remount-fs` by its cause
  rather than by its name, which is the narrowing doctor already had. The
  unit fails on a machine whose fstab still holds the `/` line Anaconda
  wrote, `kuma-fstab-sync` comments that line out on first boot, and a
  machine `kuma install` wrote has no such line at all. Skipping the unit
  by name meant the boot stage could never report it again.
- `kuma clean` reclaims the image `kuma iso --live` builds the live root
  filesystem under. It is worth nothing once the ISO is written, and
  nothing pruned it: dangling-pruning cannot, because it is tagged, and
  the stale-base rule does not cover it. It was four gigabytes.
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
