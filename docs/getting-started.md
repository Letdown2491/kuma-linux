# Getting started

There are two ways in. Download the media and install a machine, which needs
nothing but a USB stick, or build an image from a declaration on any machine
with podman. This walks both, in that order, because installing gives you a
machine that can do the building.

Each step says what to type, what you should see, and what it means. If a word
is unfamiliar, [the glossary](glossary.md) defines it in a line.

## What you need

**To install:** a USB stick, a machine to write it on, and a network
connection on the machine you are installing. Installing downloads the system
image rather than copying it off the media.

**To build your own:** [podman](https://podman.io/). That machine does not
have to run kuma, and nothing a build does changes it: an image lands in
podman's storage and nothing else is touched. Trying a declaration in a
virtual machine needs KVM and sudo on top of that.

## 1. Write the media

```console
$ curl -LO https://github.com/Letdown2491/kuma-linux/releases/latest/download/kuma-x86_64.iso
```

Around 1.8 GB. Every release asset is signed, and
[SECURITY.md](../SECURITY.md#verifying-a-release) has the one command that
checks a file came from this project's release workflow. Worth running on
something you are about to boot a machine from.

Write it to a USB stick:

```console
$ sudo dd if=kuma-x86_64.iso of=/dev/sdX bs=4M status=progress
```

Check `/dev/sdX` twice. `lsblk` lists your disks, and `dd` will overwrite
whatever you name without asking.

## 2. Look before you install

The media boots to a working desktop before anything is written to a disk,
because the ISO's root filesystem *is* a kuma image rather than an installer
program sitting beside it. Look around, open the browser, see whether the
hardware works. Nothing persists and nothing is written until you install.

## 3. Install it

Connect to a network first. On the niri desktop, `Super + T` opens a terminal.
Then:

```console
$ kuma install
```

It lists the disks it found and asks which one to use, then asks four
questions: whether to encrypt the disk, a passphrase if you say yes, an
account name and password, and a hostname. It prints the partition layout it
will write before it writes anything.

**Which image you get.** This media installs
`ghcr.io/letdown2491/kuma:niri`, the desktop image this project publishes, and
says so before it starts. Installing pulls that image from the registry rather
than copying the one you booted, which is why the network matters.

The account is asked for rather than declared because the image is shared and
you are not. Kuma writes your answers onto the target, and the machine creates
the account on its first boot.

Encryption is asked here because it cannot be added later without installing
again. Say yes and the machine asks for that passphrase at every boot, before
anything else runs. Nothing keeps a copy of it, so a lost passphrase is a lost
disk.

This is the one command in kuma that cannot be undone. There is no staged
change to discard and no rollback slot. It refuses a disk with anything
mounted on it, and without `--yes` it only prints the plan.

## 4. The first boot

Take the stick out and boot the machine. In order, you get:

1. A passphrase prompt, if you chose encryption.
2. A login screen, using the account and hostname you gave the installer.
3. A desktop, and then several minutes of quiet work.

That last part is worth expecting. The declared applications and command line
tools download on that first boot, which for a full desktop is around a
gigabyte. While it runs, `kuma` says `converging` rather than reporting that
anything is wrong:

```console
$ kuma
state: converging - flatpak convergence is running now; this is what the
machine is doing, not drift
```

When it settles, the same command says `in-sync`.

At this point you have a working machine running a declaration somebody else
wrote. The rest of this makes it yours.

## 5. Describe the machine

```console
$ kuma init
```

On the machine you just installed, that writes a copy of the declaration its
image was built from, so you start from what you have rather than from a
template. Anywhere else it writes a starter `kuma.toml` in the current
directory.

The machine already has kuma, at `/usr/bin/kuma`, put there by the build that
made its image. Do not install a second copy. On a machine that does not run
kuma, the tool is a single file and everything it needs, including the
wallpaper and the desktop configuration it bakes into images, is compiled in:

```console
$ curl -LO https://github.com/Letdown2491/kuma-linux/releases/latest/download/kuma-x86_64-unknown-linux-musl
$ chmod +x kuma-x86_64-unknown-linux-musl
$ sudo mv kuma-x86_64-unknown-linux-musl /usr/local/bin/kuma
```

The file is the whole interface. This is a complete one:

```toml
schema_version = 1

[system]
desktop = "niri"

[user]
name = "me"
shell = "fish"

[packages]
rpm = ["fish", "distrobox"]
flatpak = ["org.mozilla.firefox"]
brew = ["ripgrep", "gh"]
```

Four things are worth knowing about that file, and the rest can wait.

**Three package lists, because the three behave differently.** `rpm` is part
of the system image, so changing that list means building a new image and
rebooting into it. `flatpak` is applications, and `brew` is command line
tools; both are installed while the machine runs and need no reboot.

**A password is not in there yet.** Run `kuma passwd`, and paste what it
prints into `[user]` as `password_hash`. Without it the account exists but
cannot log in. Anyone who can read the image can read that hash, so leave it
out of anything you publish.

**You did not name a base image, and you do not have to.** Kuma builds its own
foundation out of Fedora's packages. Naming `system.base` opts out and builds
on the image you name instead.

**Your machine's own settings stay out of the file.** Hostname, timezone, and
whether a disk is encrypted belong to the machine, not to the description. Two
machines built from this one file can differ on all three.

Check it before building anything:

```console
$ kuma check
```

That validates the file and touches nothing. It is the fast way to find a
typo, and it needs no podman.

## 6. Build it and switch to it

```console
$ kuma build
```

This is the slow step, and only the first time: kuma assembles its base from
Fedora's packages, then layers your declaration on top. Later builds reuse
that base unless something in it changed. What comes out is an image named
`localhost/kuma:latest` in podman's storage, and a `kuma.lock` file beside
your declaration recording exactly what the build resolved to. Commit the lock
along with the declaration.

Building on the machine that will run the image is the shortest path from a
declaration to hardware:

```console
$ kuma switch --yes
```

That stages the image you just built. It lands when you reboot, and the system
you were on stays in the rollback slot.

**Name `podman` in `packages.rpm` if you build this way.** The published image
has it, but as a dependency of `distrobox` rather than in its own right, so a
declaration that drops distrobox and does not name podman produces a machine
that cannot build the next image.

## 7. Try changes before you commit to them

```console
$ kuma vm
```

That builds a virtual disk from the image and boots it in a window. It needs
KVM and sudo. Log in as the account you declared, or as `kuma` with the
password `kuma`, which every VM disk carries so you are never locked out of
your own test.

This is where you find out that you wanted a different terminal, or that you
forgot a package. Edit `kuma.toml`, run `kuma build` again, then:

```console
$ kuma vm --apply
```

That pushes the new image into the VM that is already running and switches it
over, keeping everything in `/var`, so your applications do not download
again. It is also the real update mechanism, which means you are testing the
thing you will later do to a real machine.

## 8. Media carrying your own declaration

The media in step 1 installs this project's published image. To hand somebody
a stick that installs *yours*:

```console
$ kuma iso --live
```

Out comes `iso/KUMA.iso`, around 1.8 GB, written the same way as step 1.

One thing decides whether it installs what you meant. Installing pulls an
image from a registry, so media built from a local `kuma build` cannot install
that build: `localhost/kuma` means nothing to the machine being installed, and
kuma installs its published image instead, saying so before it starts. Push
your image to a registry first and the media installs yours, or name it at
install time:

```console
$ kuma install --image ghcr.io/<owner>/kuma:<tag>
```

`kuma iso` without `--live` builds traditional Anaconda installer media
instead. It is about a gigabyte larger for the same system, and it needs sudo.
Use it if you want Fedora's familiar installer screens.

A `[user]` in the declaration rides into the media as a real account and
password hash, so build shareable media from a declaration without one.
Anaconda's create-a-user screen comes back on its own when you do.

## 9. Living with it

Five commands cover ordinary use. All of them read the same declaration.

```console
$ kuma                  # where this machine is, and what it can do next
$ kuma doctor           # a health check: image age, convergence, drift, encryption, disk
$ kuma update --check   # what has moved in Fedora's packages, security first
$ kuma update --yes     # build a current image and stage it for next boot
$ kuma rollback --yes   # go back to the deployment you were on before
```

Running bare `kuma` is always safe and always tells you where you are. Every
command ends by naming what you can legally do next, so you can follow the
prompts rather than remember the verbs.

**On a desktop, `Mod+D` opens kuma's menu.** Your applications are in it, so
it is the launcher; so are the settings tools kuma does not own (network,
bluetooth, audio), the ones it does (your declaration, health, updates,
rollback), and lock, suspend, reboot and power off. Opening it shows the
groups and typing searches every row, so `reboot` finds the reboot without
going anywhere. It never writes your declaration: those rows open the file or
run a command that asks first. To read it without a desktop:

```console
$ kuma menu --list      # the rows the menu would offer on this machine
```

**When something is wrong and you want help.** `kuma doctor --report` prints
one JSON document with the findings, which kuma is running, which image is
booted and its digest, and the declaration the machine was built from. That is
what to attach to a bug report. The password hash is removed before it prints,
a declaration kuma cannot parse is left out entirely rather than pasted raw,
and where a report quotes what a failed service said, anything shaped like a
password hash is masked in that too.

**Updates never happen behind your back.** Kuma tells you when there is
something to take and leaves the taking to you. `kuma update --yes` builds a
new image and stages it; the change lands when you reboot, and the previous
system stays in the rollback slot. If an update would move you to a new Fedora
release, it says so in those words before anything is staged, because that is
the largest change kuma can make to a machine and it otherwise arrives as
several hundred package lines.

**A machine tracking the published image checks the signature.** Every image
ships kuma's signing key and a policy requiring it, so an update that did not
come from this project is refused rather than installed. `kuma doctor` grades
that the policy is really in place; images you build yourself are your own and
are not required to be signed.

**A bad update rolls itself back.** If a new image fails to boot to a working
desktop three times, the bootloader falls back to the previous one on its own.
You do not have to be there.

**Changes you make by hand are not errors.** Install something from a store,
or with `brew install`, and kuma leaves it alone. `kuma diff` shows what your
machine has that your file does not mention, and `kuma capture` offers to
write it into the declaration for you. Nothing is deleted for being
undeclared.

## Where to go next

- [How kuma behaves](concepts.md) explains the reasoning under all of this:
  what is pinned, what merges, how rollback works, and why drift is treated as
  a proposal.
- [What a desktop contains](desktops.md) lists what `desktop = "niri"` or
  `"cosmic"` installed that you never named.
- [Glossary](glossary.md) for any word above that was new.
