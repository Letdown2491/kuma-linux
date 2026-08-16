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

**Digest.** A checksum that names an exact image, as opposed to a tag like
`:44`, which points at whatever was published most recently. `kuma.lock`
pins a digest so a build is repeatable.

**Drift.** Anything the machine has that the declaration does not name. Kuma
treats it as a proposal to consider rather than a fault to erase; see
[how kuma behaves](concepts.md#what-happens-to-changes-you-make-by-hand).

**Flatpak.** A packaged desktop application that carries its own
dependencies and updates independently of the system. Kuma's `packages.flatpak`
list installs these from Flathub, and they need no reboot.

**Homebrew, brew.** A package manager for command line tools that installs
into your home directory rather than into the system. Kuma's
`packages.brew` list uses it, and it needs no reboot.

**greenboot.** The health check that runs on the first boot of a new
system. If it fails three times, the bootloader falls back to the previous
deployment on its own.

**LUKS.** Linux disk encryption. `kuma install` can put the root filesystem
inside a LUKS container, which the machine unlocks with a passphrase at
every boot.

**ostree.** The technology underneath bootc that stores the system
read-only and merges your `/etc` onto each new deployment. It is why a file
you edited by hand keeps winning over later images, and why `kuma doctor`
watches for that.

**Podman.** The container tool kuma uses to build. Everything `kuma build`
does is podman doing ordinary work, and it needs no root.

**rpm.** A Fedora package. Kuma's `packages.rpm` list becomes part of the
system image, so changing it means building a new image and rebooting.

**Snapshot.** A read-only copy of `/var/home` as it was at a moment, taken
hourly when `[snapshots]` is enabled. Cheap, because btrfs shares the
unchanged data rather than copying it.

**Stage, staged.** A new deployment written to the disk and set to boot next
time, without touching what is running. `kuma update --yes` stages; the
reboot is yours to choose.

**Subvolume.** A btrfs filesystem within a filesystem, which can be
snapshotted on its own. Kuma installs the system into one named `root`.

**Tag.** A moving name for an image, like `fedora-bootc:44`. What it points
at changes when a new one is published, which is why a build records the
digest instead.
