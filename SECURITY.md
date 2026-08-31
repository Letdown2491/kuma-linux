# Security

Kuma is early and maintained by one person.

Four things here are worth reading even if you never report a bug: what
naming a package in your declaration opts you into, how to check that the
binary you downloaded came from this project, what a password hash in a
declaration exposes, and what disk encryption does and does not protect.

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

- **`packages.rpm`** comes from Fedora's repositories. You cannot declare a
  third-party repository; there is no key for it. Signature checking is dnf's
  default and kuma never disables it, and a name that tries to become a flag
  (`rpm = ["--nogpgcheck"]`) is rejected before it reaches dnf.
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
- **`system.ca_certificates`** is trust in a certificate authority, and it is
  the most direct entry on this list: a certificate named there is trusted for
  every TLS connection the machine makes, by every program that reads the
  system trust store. It is copied to `/etc/pki/ca-trust/source/anchors/` and
  `update-ca-trust` runs in the same build layer. `kuma check` rejects a value
  that is not a PEM certificate, and rejects one containing a private key
  outright rather than warning, because a key there would be baked
  world-readable into every image built from that declaration.

Names in these lists are validated before they reach dnf, flatpak, systemctl,
or brew: no leading dashes, so a name can't become a flag, and no shell
metacharacters. `rpm = ["--nogpgcheck"]` and `rpm = ["fish; rm -rf /"]` are
both rejected by `kuma check`.

### Two roots your declaration does not name

Choosing a desktop brings in two package sources beyond Fedora's own, and
neither appears in the list above because neither is something you asked for by
name. They are listed here rather than left for you to find in a build log.

- **RPM Fusion**, on every desktop build. Fedora's `mesa-va-drivers` ships with
  H.264/H.265/VC-1 decode stripped for patent reasons, so video silently falls
  back to the CPU; kuma installs RPM Fusion's `mesa-va-drivers-freeworld`
  instead. Getting there means installing `rpmfusion-free-release` from a URL,
  which is the bootstrap every third-party Fedora repository has: the package
  that carries the signing key cannot itself be checked against it. dnf reports
  this as `skipped OpenPGP checks for 1 package`. Everything afterwards,
  including the driver itself, is checked against RPM Fusion's key.
- **`fedora-cisco-openh264`**, which Fedora enables by default and which reaches
  the image because the desktop layer installs weak dependencies. It is hosted
  by Cisco rather than Fedora.

Both are the same trust decision Fedora Workstation makes for the same reason,
and a `minimal` declaration reaches neither. If you want a machine that trusts
only Fedora, declare no desktop.

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
it goes on to build the filesystem you boot. That is the same command every
release's notes carry, deliberately: a release's notes cannot be corrected
once people have them, so the two say one thing.

The installer media on the same page is signed the same way:

```console
$ cosign verify-blob \
    --bundle kuma-x86_64.iso.bundle \
    --certificate-identity-regexp '^https://github.com/Letdown2491/kuma-linux/' \
    --certificate-oidc-issuer https://token.actions.githubusercontent.com \
    kuma-x86_64.iso
```

Worth more, if anything. The binary builds a system you then choose to boot;
this file boots one directly, on hardware, before you have anything to
inspect it with. The `.sha256` beside it is not a substitute, because it is
served from the same page: whatever could replace one could replace both.

Published images are signed with a key pair instead, and `cosign.pub` in this
repository is the public half:

```console
$ cosign verify --key cosign.pub ghcr.io/letdown2491/kuma:niri
```

The two differ for a reason rather than by accident. A `policy.json`
`sigstoreSigned` requirement takes exactly one of `keyPath`, `fulcio` or
`pki`, and the `fulcio` block requires both `oidcIssuer` and `subjectEmail`.
A GitHub Actions certificate carries no email, only a URI SAN naming the
workflow, so no policy file can express "signed by kuma's release workflow".
An image people configure a machine to trust needs a key a policy can name; a
blob a person verifies once by hand does not.

**Your machine checks that signature without being asked.** Every kuma image
ships the key at `/etc/pki/containers/kuma.pub` and a `/etc/containers/policy.json`
requiring a valid kuma signature for `ghcr.io/letdown2491/kuma`, so an update
that did not come from kuma is refused rather than installed. `kuma doctor`
grades this, because a signature nobody checks is a claim and not a control.

The rule is deliberately narrow. That policy file is shared by podman and
bootc, so requiring signatures everywhere would refuse Fedora's base image on
your next `kuma update` and refuse your own locally built image on your next
`kuma switch`. Everything other than kuma's own published repository is left
as it was.

Two consequences worth knowing. The identity is matched at repository level,
because that is what cosign records: a signature is accepted for
`ghcr.io/letdown2491/kuma` regardless of which tag you pull, so anything kuma
signed and published is trusted by any machine tracking that repository. And
images you build yourself are not signed and are not required to be; they come
from your own storage, which the policy leaves alone.

Publishing is refused without a signature unless somebody explicitly asks for
it, and the workflow verifies its own output against the committed public key
before finishing, because a `cosign.pub` that does not match the signing
secret fails silently: the image publishes and verification fails everywhere
else.

The private half of the key lives in GitHub's secret store; only the public
half is committed, and the check above is what ties the two together. What
happens to that key is worth saying plainly:

**Losing it costs what is unpublished, nothing published.** Signatures live
in the registry, so machines keep verifying and keep upgrading within what
has already been published. Nothing new can be signed, and a machine whose
policy names the lost key cannot receive a new policy by update, because
the update is exactly what that policy refuses. Adopting a new key is a
deliberate step on each machine, which is the design refusing to make key
adoption silent.

**Rotating while the key is held can ride an update.** Images can be signed
with both keys while the policy names both, so the new policy reaches
machines as an ordinary verified update, and the old key retires once no
machine still requires it.

**Rotating because the key is compromised has no in-band path, by design.**
Any path that swapped a machine's key on its own is the path an attacker
holding the key would use too, so a new key reaches machines the way the
first one did: deliberately, by the person responsible for the machine.

Images kuma builds for you are not signed. They're built on your machine, from
your declaration, and stay in your local container storage unless you push
them somewhere. What these guarantees promise and what a 2.0 may change is
stated in docs/contract.md.

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

### That hash is reachable from the network

The paragraphs above discuss cracking the hash offline, from a published image.
There is a second way to meet it, and this document used to leave it out.

**Every kuma image enables `sshd`, and the firewall lets it through.** The
default zone is `public`, which permits the `ssh` service, and nothing kuma
writes changes OpenSSH's own defaults, so password authentication is on. The
account `kuma install` creates is in `wheel`. On a laptop that joins a network
you do not run, that is a password prompt anybody on that network can reach.

The hash itself is calibrated for the offline case: sha512-crypt at 656,000
rounds, which is expensive per guess. Online guessing is a different problem,
and OpenSSH's answer to it is rate limiting rather than lockout, so a weak
password is weak here in a way the round count does not help with.

If you do not want that, either turn it off in the declaration:

```toml
[services]
disable = ["sshd.service"]
```

or leave it on and make sure the account has a password worth having, or set
`user.ssh_keys` and turn password authentication off yourself. Kuma ships the
distribution default rather than choosing for you, and it says so here rather
than leaving you to find out.

### The one secret a declaration deliberately does not carry

`[backup]` needs a credential for its repository, and `backup.secret` names it
rather than holding it. The value lives at `/var/lib/kuma/secrets/<name>.env`,
mode 0600, owned by root, put there by hand. A declaration is written to be
committed and is baked world-readable into every image built from it, which is
the same reasoning that keeps a password hash out of a published image, applied
before the fact rather than after.

A repository address carrying its own password (`s3:https://KEY:SECRET@host/…`)
is **refused by `kuma check`**, not warned about. That does not happen because
somebody decides to put a secret in git; it happens because a restic command
line that already worked gets pasted in.

**What the far end can see.** restic encrypts and authenticates client-side, so
the repository holds ciphertext and the server storing it cannot read your
files, whoever runs it. What it does see is the shape of the traffic: how much
you store, how often, and when. Treat the repository password as protecting the
data and nothing else as protecting the metadata.

**`network_connections` moves wifi passphrases off the machine.** Those files
hold a passphrase per network in the clear, so backing them up puts them in the
repository, encrypted with everything else. That is why the key is off by
default and why `kuma doctor` names which way it is set on every run: it should
be a decision you made, not one made for you.

**A restore carries the credential onto the target.** `kuma install --restore`
writes it into `/var/lib/kuma/secrets/restore.env` on the new machine through
an image layer, and that layer is built into a temporary subvolume the install
deletes on the way out, the same handling the account's password hash already
gets.

## Disk encryption

`kuma install` asks whether to encrypt the disk, and encrypts nothing unless
told to. Saying yes puts a LUKS2 container in the root partition, holding the
same btrfs root, and the machine asks for the passphrase at every boot before
anything else runs.

The question is only put to a terminal. An install driven from a pipe or a
script is unencrypted unless it passes `--encrypt`, which is the answer that
can be undone: an unencrypted machine can be reinstalled, and one whose
passphrase nobody chose cannot be booted.

**Kuma writes the passphrase nowhere.** It reaches `cryptsetup` on a pipe,
never through a command line where `ps` would show it and never through a
file. A lost passphrase is a lost disk; there is no recovery key, no escrow,
and no way for kuma to help. Changing it later is `cryptsetup luksChangeKey`
on the machine itself, which kuma has no verb for.

It does live in memory while the install runs: in kuma's own heap, and in a
shell variable in the install script. Neither is scrubbed, and kuma makes no
claim to defend against something reading another process's memory, which on
this machine already means root. What it defends is the disk you are holding.

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
for somebody else. When the account being created is not the one the image
declares, the installer also drops that account's autologin from the greeter,
since a greeter cannot log in a user the machine will not have. Install an
image for the account it already declares and the autologin stays, because
then it names somebody who exists.

`kuma iso --live` does not carry the declared account into the live session:
it creates its own passwordless `liveuser` with passwordless sudo, which
exists only inside the ISO's read-only filesystem and never reaches an
installed machine. A declared `[user]` is still baked into the image the ISO
was built from, so the sentence above still applies to what gets installed.
The live session also runs SELinux permissive, for the reason recorded in
`src/liveiso.rs`; an installed machine is enforcing from its first boot.

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
history: on a terminal it is prompted for, and from a pipe it is read as one
line, with the account name coming from `--user`, which is required there. An
encrypted install reads two lines, the disk passphrase first. The
hash is the same sha512-crypt at 656k rounds `kuma passwd` produces. Unlike a
declared `[user]`, it is written to the machine rather than baked into the
image, so it is not published by publishing the image.

## Not yet

- Builds are not reproducible, and kuma makes no claim that two builds of one
  declaration produce identical bytes.
- Kuma emits no SBOM. `kuma.lock` records resolved package versions, which is
  adjacent but not the same thing.
- Images built from your own declaration are not signed and are not required
  to be; they come from your own storage, which the policy leaves alone. The
  base image is unsigned, and the policy deliberately leaves it alone too:
  the digest pin in `kuma.lock` is the only check on it.
- There is no security advisory history, because there have been no advisories.
