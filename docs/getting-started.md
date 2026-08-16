# Getting started

This walks the whole path once: install the tool, describe a machine, try it
somewhere safe, then put it on real hardware and keep it current. Each step
says what to type, what you should see, and what it means.

If a word is unfamiliar, [the glossary](glossary.md) defines it in a line.

## What you need

One machine to build on, with [podman](https://podman.io/) installed. That
machine does not have to run kuma, and nothing you do here changes it: a
build produces an image in podman's storage and touches nothing else.

Two steps below ask for more. Trying a declaration in a virtual machine
needs KVM and sudo. Writing installer media needs neither, which is
deliberate: media you can build without a password is media you will
actually build.

## 1. Install kuma

Kuma is a single file. Everything it needs, including the wallpaper and the
desktop configuration it bakes into images, is compiled in.

```console
$ curl -LO https://github.com/Letdown2491/kuma-linux/releases/latest/download/kuma-x86_64-unknown-linux-musl
$ chmod +x kuma-x86_64-unknown-linux-musl
$ sudo mv kuma-x86_64-unknown-linux-musl /usr/local/bin/kuma
```

Every release is signed, and [SECURITY.md](../SECURITY.md#verifying-a-release)
has the one command that checks the file came from this project's release
workflow. Worth running: this binary goes on to build the system you boot.

Already running a kuma machine? It has kuma at `/usr/bin/kuma` already, put
there by the build that made its image. Do not install a second copy.

## 2. Describe the machine

```console
$ kuma init
```

That writes a starter `kuma.toml` in the current directory. On a machine
already running kuma, it writes a copy of that machine's own declaration
instead, so you start from what you have rather than from a template.

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

**You did not name a base image, and you do not have to.** Kuma builds its
own foundation out of Fedora's packages. Naming `system.base` opts out and
builds on the image you name instead.

**Your machine's own settings stay out of the file.** Hostname, timezone,
and whether a disk is encrypted belong to the machine, not to the
description. Two machines built from this one file can differ on all three.

Check it before building anything:

```console
$ kuma check
```

That validates the file and touches nothing. It is the fast way to find a
typo, and it needs no podman.

## 3. Build it

```console
$ kuma build
```

This is the slow step, and only the first time: kuma assembles its base from
Fedora's packages, then layers your declaration on top. Later builds reuse
that base unless something in it changed. What comes out is an image named
`localhost/kuma:latest` in podman's storage, and a `kuma.lock` file beside
your declaration recording exactly what the build resolved to. Commit the
lock along with the declaration.

## 4. Try it before you commit to it

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

That pushes the new image into the VM that is already running and switches
it over, keeping everything in `/var`, so your applications do not download
again. It is also the real update mechanism, which means you are testing the
thing you will later do to a real machine.

## 5. Make installer media

```console
$ kuma iso --live
```

Out comes `iso/KUMA.iso`, around 1.8 GB. Write it to a USB stick:

```console
$ sudo dd if=iso/KUMA.iso of=/dev/sdX bs=4M status=progress
```

Check `/dev/sdX` twice. `lsblk` lists your disks, and `dd` will overwrite
whatever you name without asking.

The media boots to a working desktop before anything is written to a disk,
because the ISO's root filesystem *is* your image rather than an installer
program sitting beside it. Look around, open the browser, see whether the
hardware works. Nothing persists and nothing is written until you install.

`kuma iso` without `--live` builds traditional Anaconda installer media
instead. It is about a gigabyte larger for the same system, and it needs
sudo. Use it if you want Fedora's familiar installer screens.

## 6. Install it

Boot the stick, and connect it to a network: installing downloads the system
image rather than copying it off the media. On the niri desktop, `Super + T`
opens a terminal. Then:

```console
$ kuma install
```

It lists the disks it found and asks which one to use, then asks four
questions: whether to encrypt the disk, a passphrase if you say yes, an
account name and password, and a hostname. It prints the partition layout it
will write before it writes anything.

**Which image you get.** The live session is running the image you built, but
`kuma install` writes the published `ghcr.io/letdown2491/kuma:niri` unless
`--image` names another. Installing pulls from a registry, and an image you
built on your own machine is not in one. So there are two ways to end up
running your own declaration on real hardware: push your image somewhere and
name it with `--image`, or install the published one and then build yours on
the machine itself with `kuma build` and `kuma switch --yes`, which needs
`podman` in your `packages.rpm`.

The account is asked for rather than declared because the image is shared
and you are not. Kuma writes your answers onto the target, and the machine
creates the account on its first boot.

Encryption is asked here because it cannot be added later without installing
again. Say yes and the machine asks for that passphrase at every boot,
before anything else runs. Nothing keeps a copy of it, so a lost passphrase
is a lost disk.

This is the one command in kuma that cannot be undone. There is no staged
change to discard and no rollback slot. It refuses a disk with anything
mounted on it, and without `--yes` it only prints the plan.

## 7. The first boot

Take the stick out and boot the machine. In order, you get:

1. A passphrase prompt, if you chose encryption.
2. A login screen, using the account and hostname you gave the installer.
3. A desktop, and then several minutes of quiet work.

That last part is worth expecting. The declared applications and command
line tools download on that first boot, which for a full desktop is around a
gigabyte. While it runs, `kuma` says `converging` rather than reporting that
anything is wrong:

```console
$ kuma
state: converging - flatpak convergence is running now; this is what the
machine is doing, not drift
```

When it settles, the same command says `in-sync`.

## 8. Living with it

Five commands cover ordinary use. All of them read the same declaration.

```console
$ kuma                  # where this machine is, and what it can do next
$ kuma doctor           # a health check: image age, drift, encryption, disk
$ kuma update --check   # what has moved in Fedora's packages, security first
$ kuma update --yes     # build a current image and stage it for next boot
$ kuma rollback --yes   # go back to the deployment you were on before
```

Running bare `kuma` is always safe and always tells you where you are. Every
command ends by naming what you can legally do next, so you can follow the
prompts rather than remember the verbs.

**Updates never happen behind your back.** Kuma tells you when there is
something to take and leaves the taking to you. `kuma update --yes` builds a
new image and stages it; the change lands when you reboot, and the previous
system stays in the rollback slot.

**A bad update rolls itself back.** If a new image fails to boot to a working
desktop three times, the bootloader falls back to the previous one on its
own. You do not have to be there.

**Changes you make by hand are not errors.** Install something from a store,
or with `brew install`, and kuma leaves it alone. `kuma diff` shows what your
machine has that your file does not mention, and `kuma capture` offers to
write it into the declaration for you. Nothing is deleted for being
undeclared.

## Where to go next

- [How kuma behaves](concepts.md) explains the reasoning under all of this:
  what is pinned, what merges, how rollback works, and why drift is treated
  as a proposal.
- [What a desktop contains](desktops.md) lists what `desktop = "niri"` or
  `"cosmic"` installed that you never named.
- [Glossary](glossary.md) for any word above that was new.
