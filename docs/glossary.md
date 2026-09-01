# Glossary

The words kuma's documentation uses, in one line each. Where a term has a
general meaning and a narrower one here, the narrow one is what kuma means.

**Atomic.** A change that either fully happens or does not happen at all.
Kuma never edits a running system in place: it builds a whole new system
image and switches to it at the next boot, so there is no half-updated
state to be stuck in.

**Base.** The foundation an image is built on. With `system.base` unset,
kuma composes its own from Fedora's packages rather than starting from
somebody else's image.

**bootc.** The Fedora technology kuma builds on, which lets a machine boot
from a container image and update by swapping that image. Kuma's job is to
produce the image; bootc's is to boot it and to roll it back.

**Container image.** A packaged filesystem, the same format that runs
containers. A bootc machine boots one instead of running it, so "image"
throughout these docs means the whole operating system, not an application.

**Converge, convergence.** Making the machine match the declaration:
installing what is named, updating what is present, removing what kuma
itself installed and the declaration no longer names. It runs at boot and on
a daily timer for applications and command line tools.

**Declaration.** Your `kuma.toml`. The file that says what the system is.

**Deployment.** One bootable system on the machine. A bootc machine keeps
more than one, which is what makes rollback instant: the previous deployment
is still there.

**Rebase.** Pointing a bootc machine at a different image, so the next boot
runs that image's system while everything in `/var` stays where it was. The
way an existing machine moves to kuma without losing its home directory; see
[moving over](moving.md).

**Digest.** A checksum that names an exact image, as opposed to a tag like
`:44`, which points at whatever was published most recently. `kuma.lock`
pins a digest so a build is repeatable.

**ESP.** The EFI system partition: the small FAT partition the firmware
reads the bootloader from, before the operating system exists. The first
of the three partitions an install writes, and the one `--esp` sizes.

**Drift.** Anything the machine has that the declaration does not name. Kuma
treats it as a proposal to consider rather than a fault to erase; see
[how kuma behaves](concepts.md#what-happens-to-changes-you-make-by-hand).

**Capture.** The verb that acts on drift: `kuma capture` proposes declaring
what the machine already runs, and writes nothing until `--yes`. It covers
the whole mutable edge, flatpaks and brew leaves and the flatpak permissions
the machine carries that the declaration does not name, and it never touches
the machine, only the file.

**Flatpak.** A packaged desktop application that carries its own
dependencies and updates independently of the system. Kuma's `packages.flatpak`
list installs these from Flathub, and they need no reboot.

**Homebrew, brew.** A package manager for command line tools that installs
into your home directory rather than into the system. Kuma's
`packages.brew` list uses it, and it needs no reboot.

**greenboot.** The health check that runs on the first boot of a new
system. If it fails three times, the bootloader falls back to the previous
deployment on its own.

**Hibernate.** Writing memory to disk and powering off, so the machine comes
back where it was. It needs a swapfile, which kuma can make at install or with
`kuma hibernate`, and it needs the kernel told where that file sits on the
disk. Distinct from suspend, which keeps memory powered and needs none of
this. Not the same as **zram**, which is swap inside memory and therefore
cannot hold a copy of it. Unavailable on a machine that booted with Secure
Boot, because the kernel is then locked down; see **Lockdown**.

**Suspend-then-hibernate.** Suspending first and hibernating only when the
battery demands it, which is what closing the lid does on a kuma machine with
a swapfile: the firmware's low-battery alarm wakes the machine just before it
would die, and it hibernates then. The session survives either way, and the
battery survives the bag.

**Plymouth.** The program that owns the screen between the firmware and the
login screen, so boot messages need not flash by in text. Kuma's image builds
it into the initramfs and gives it a spinner theme, which is also what draws
the encrypted disk's passphrase prompt.

**Lockdown.** A kernel mode that blocks operations able to read or write kernel
memory. Booting with Secure Boot turns it on in `integrity` mode, and one of
the things it blocks is hibernation, since a hibernate image is a way to write
memory back into a running kernel. Nothing kuma configures changes it: it is
decided by the firmware setting and the kernel Fedora ships.

**Installer media.** The USB stick a machine boots to be installed. Kuma's is
live: its root filesystem is the desktop image itself, so what you look at
before installing is what you get, and nothing is written until you say so.

**LUKS.** Linux disk encryption. `kuma install` can put the root filesystem
inside a LUKS container, which the machine unlocks with a passphrase at
every boot.

**Machine state.** What is true of one machine rather than of the system it
runs: which wifi network is joined, which speaker is paired, the volume, the
hostname, the timezone. Kuma deliberately does not put these in the
declaration, because a file that described them could not be shared, and
tools own them instead. The opposite of **system definition**.

**ostree.** The technology underneath bootc that stores the system
read-only and merges your `/etc` onto each new deployment. It is why a file
you edited by hand keeps winning over later images, and why `kuma doctor`
watches for that.

**Podman.** The container tool kuma uses to build. Everything `kuma build`
does is podman doing ordinary work, and it needs no root.

**rpm.** A Fedora package. Kuma's `packages.rpm` list becomes part of the
system image, so changing it means building a new image and rebooting.

**Override.** A permission you granted or took away from a flatpak, stored
in a file flatpak keeps and kuma shares. `[overrides]` declares them per app,
and convergence sets only the keys you declared, so a permission you toggle in
Flatseal survives unless your declaration says otherwise.

**Signature.** Proof that an image was published by this project rather
than by whoever else could reach the registry. Every image carries kuma's
signing key and a policy requiring it, so an update whose signature does not
check out is refused instead of installed, and `kuma doctor` grades the
policy rather than trusting that it is there.

**Snapshot.** A read-only copy of `/var/home` as it was at a moment, taken
hourly when `[snapshots]` is enabled. Cheap, because btrfs shares the
unchanged data rather than copying it.

**Backup.** A copy of a snapshot in a restic repository somewhere else,
made on a timer when `[backup]` is enabled. Snapshots survive a mistake;
backups survive the disk. The declaration names the credential and does
not hold it, so recovering a machine takes two things: the file, and the
credential the file names.

**Stage, staged.** A new deployment written to the disk and set to boot next
time, without touching what is running. `kuma update --yes` stages; the
reboot is yours to choose.

**Subvolume.** A btrfs filesystem within a filesystem, which can be
snapshotted on its own. Kuma installs the system into one named `root`.

**System definition.** What is true of every machine built from a
declaration: which packages, which desktop, which firmware, which shell.
This is what `kuma.toml` describes, and changing it means a build and a
reboot rather than an immediate effect. The opposite of **machine state**.

**Tag.** A moving name for an image, like `fedora-bootc:44`. What it points
at changes when a new one is published, which is why a build records the
digest instead.

**Contract.** What kuma 44.0 promises about every surface above: the
declaration format, the verbs and flags, the `--json` documents, the
published images. Additions arrive freely; the major version names the
Fedora base and is not a promise boundary. See [the contract](contract.md).
