# Security

Kuma is early and maintained by one person.

## Reporting a vulnerability

Use GitHub's private vulnerability reporting:
[**Report a vulnerability**](https://github.com/Letdown2491/kuma-linux/security/advisories/new).
It reaches the maintainer privately, and it is the only channel. Please don't
open a public issue for one first.

Worth including: the output of `kuma --version`, the declaration that
reproduces it with any password hash removed, and what an attacker ends up
able to do.

Expect a reply in days rather than hours. There is no bounty and no response
guarantee. Only the most recent release is supported; fixes go out in a new
release rather than as backports to older tags.

## What is kuma's to fix

Kuma compiles a declaration into a Containerfile and hands it to podman. It
builds no packages and no kernels. What it adds to an image is its own binary,
the systemd units it writes, and the desktop assets compiled into it.

So a flaw in the kernel, in systemd, in a Fedora package, or in bootc is not
kuma's to patch, and kuma needs no patch mechanism of its own: a rebuild
resolves against Fedora's packages as they are that day, which makes updating
and patching the same operation. `kuma update` is how you take security
updates.

A flaw in how kuma generates a build, in what it puts in an image, or in what
it runs on your machine is kuma's, and is worth reporting.

## Your declaration is the trust boundary

One short file spans several trust roots, and naming a string is how you opt
into each:

- **`packages.rpm`** comes from Fedora's repositories. Kuma adds no third-party
  repositories and provides no way to declare one. Signature checking is dnf's
  default and kuma never disables it.
- **`system.base`**, when set, is trust in whoever publishes that image. Unset,
  kuma composes a base from Fedora's repositories instead, so the trust root is
  the same as for `packages.rpm`.
- **`packages.flatpak`** is trust in Flathub and in each application's
  publisher. These converge on every boot, as root.
- **`packages.brew`** is trust in Homebrew and in each formula's upstream.
  Naming any formula (or setting `system.brew`) makes the image fetch
  Homebrew's tarball over HTTPS on first boot, with no signature to check
  because Homebrew publishes none.
  Formulae then install into `/home/linuxbrew`, owned by your user, rather than
  into the image.
- **`services.enable`** starts units that are already in the image. It cannot
  introduce one.

Names in these lists are validated before they reach dnf, flatpak, systemctl,
or brew: no leading dashes, so a name can't become a flag, and no shell
metacharacters. `rpm = ["--nogpgcheck"]` and `rpm = ["fish; rm -rf /"]` are
both rejected by `kuma check`.

## What a build pins

`kuma.lock` records what a build resolved. The base digest is enforced, so the
same declaration and the same lock build from the same bytes. Package versions
are recorded but not enforced, because Fedora's mirrors garbage collect old
builds within weeks and a pinned version would become a build failure rather
than a defense. The record is there to show what moved between two builds.

`kuma update` is the only thing that moves the pin.

## Verifying a release

Every release asset is signed with Sigstore, keyless, using the release
workflow's own identity. No private key exists to be stolen; what an attacker
would have to take is push access to this repository.

```console
$ cosign verify-blob \
    --bundle kuma-x86_64-unknown-linux-musl.bundle \
    --certificate-identity-regexp '^https://github.com/Letdown2491/kuma-linux/' \
    --certificate-oidc-issuer https://token.actions.githubusercontent.com \
    kuma-x86_64-unknown-linux-musl
```

Worth doing: the install instructions put this binary in `/usr/local/bin` and
it goes on to build the filesystem you boot.

Images kuma builds for you are not signed. They're built on your machine, from
your declaration, and stay in your local container storage unless you push
them somewhere.

## Secrets in a declaration

`user.password_hash` is baked into the image. Anyone who can pull that image
can read the hash and start cracking it offline. That is fine for an image
that never leaves your machine and bad for one you publish, so **don't push an
image built from a declaration that carries a password hash.** The committed
examples declare no user for this reason.

`user.ssh_keys` holds public keys and is safe to publish.

`user.autologin` means the machine boots to a session with no password prompt.
It's a deliberate choice for a kiosk or a VM, and it is not a good one for a
laptop that leaves the house.

## Disk encryption

`kuma install` asks whether to encrypt the disk, and encrypts nothing unless
told to. Saying yes puts a LUKS2 container in the root partition, holding the
same btrfs root, and the machine asks for the passphrase at every boot before
anything else runs.

**Kuma keeps no copy of the passphrase.** It reaches `cryptsetup` on a pipe,
never through a command line where `ps` would show it and never through a
file. A lost passphrase is a lost disk; there is no recovery key, no escrow,
and no way for kuma to help. Changing it later is `cryptsetup luksChangeKey`
on the machine itself, which kuma has no verb for.

**What it protects is a disk at rest, and only that.** `/boot` and the EFI
system partition are outside the container on every install, because a
bootloader has to read a kernel before anything is unlocked. Nothing measures
or verifies them, so an attacker with repeated physical access can modify the
initramfs that later asks for your passphrase. Defending against that needs
Secure Boot with signed and measured boot, which kuma does not do. Encryption
here answers a stolen or discarded disk, not a machine somebody keeps
visiting.

Encryption is not a field in `kuma.toml`, deliberately. It is a property of a
disk, fixed when that disk is partitioned, and two machines built from one
declaration can differ on it. Machine state stays out of the declaration for
the same reason hostname and timezone do.

## VM and installer images

`kuma vm` disks carry a `kuma` account with the password `kuma` and membership
in `wheel`, so a freshly built VM is always reachable. QEMU forwards its ssh
port on `127.0.0.1` only. Treat a `kuma vm` guest as a scratch machine and
don't expose one to a network.

`kuma iso` builds installer media from your declaration, and a declared
`[user]` rides along into it, password hash included. `kuma iso` says so when
it happens. Build shareable media from a declaration with no `[user]`.

The same is true of the images themselves, and `kuma install` says so too: the
declaration is baked into every image, so installing one that declares a
`[user]` writes that account's name and password hash onto a disk being made
for somebody else. Installing an image that declares an account also drops
that account's autologin from the greeter, since a greeter cannot log in a
user the installed machine will not have.

`kuma iso --live` does not carry the declared account into the live session:
it creates its own passwordless `liveuser` with passwordless sudo, which
exists only inside the ISO's read-only filesystem and never reaches an
installed machine. A declared `[user]` is still baked into the image the ISO
was built from, so the sentence above still applies to what gets installed.
The live session also runs SELinux permissive, for the reason recorded in
`src/liveiso.rs`; an installed machine is enforcing from its first boot.

## Verifying what you downloaded

The release binary is signed keylessly, so there is no key to trust, only a
workflow identity:

```
cosign verify-blob \
  --bundle kuma-x86_64-unknown-linux-musl.bundle \
  --certificate-identity-regexp '^https://github.com/Letdown2491/kuma-linux/' \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com \
  kuma-x86_64-unknown-linux-musl
```

That is the same command every release's notes carry, deliberately: a
release's notes cannot be corrected once people have them, so the two say
one thing.

Published images are signed with a key pair instead, and `cosign.pub` in
this repository is the public half:

```
cosign verify --key cosign.pub ghcr.io/letdown2491/kuma:niri
```

The two differ for a reason rather than by accident. A `policy.json`
`sigstoreSigned` requirement takes exactly one of `keyPath`, `fulcio` or
`pki`, and the `fulcio` block requires both `oidcIssuer` and `subjectEmail`.
A GitHub Actions certificate carries no email, only a URI SAN naming the
workflow, so no policy file can express "signed by kuma's release workflow".
An image people configure a machine to trust needs a key a policy can name;
a blob a person verifies once by hand does not.

Publishing is refused without a signature unless somebody explicitly asks
for it, and the workflow verifies its own output against the committed
public key before finishing, because a `cosign.pub` that does not match the
signing secret fails silently: the image publishes and verification fails
everywhere else.

## What runs as root

`init`, `check`, `generate`, and `build` need only rootless podman.

`switch`, `update`, `rollback`, and `sync` call `bootc` and `systemctl` under
sudo. `vm` and `iso` need sudo because bootc-image-builder runs as root, with
one exception: `iso --live` never calls it, and so never asks. `install` runs
`bootc install` in a privileged container with `/dev` bound in, which is what
writing a disk requires. Kuma asks for sudo at those points and nowhere else.

`kuma install` asks for a password and writes its hash to
`/var/lib/kuma/user` on the target, mode 0600, where `kuma-user-sync` reads
it at first boot. `/var` rather than `/etc` because bootc fills `/var` from
the image once at install and never touches it again, while `/etc` is
three-way merged on every update: a file an installer shipped as image
content is not a local modification, so merging against a published image
that has no such file would delete it. The
password is never passed as an argument, so it does not reach `ps` or a shell
history; piped stdin takes the account name and password as two lines. The
hash is the same sha512-crypt at 656k rounds `kuma passwd` produces. Unlike a
declared `[user]`, it is written to the machine rather than baked into the
image, so it is not published by publishing the image.

## Not yet

- Builds are not reproducible, and kuma makes no claim that two builds of one
  declaration produce identical bytes.
- Kuma emits no SBOM. `kuma.lock` records resolved package versions, which is
  adjacent but not the same thing.
- Images kuma builds are not signed, and kuma has no verification step for a
  `system.base` beyond the digest pin in `kuma.lock`.
- There is no security advisory history, because there have been no advisories.
