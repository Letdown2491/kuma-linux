# Contributing

**Smoke tests.** `scripts/smoke.sh` builds every committed example and, with
`--boot`, boots it. Three stages, cheapest first: `check` validates the
declaration, `image` builds it and inspects what a successful build doesn't
already prove, and `boot` makes a disk, boots it headless, and asks the
machine whether the boot was healthy. That last verdict is greenboot's own,
so the check that would roll an update back is the one that decides whether
the test passed.

```console
$ cargo test                    # the tier that needs no machine
$ cargo fmt                     # rustfmt.toml settles layout; CI checks it
$ scripts/smoke.sh              # check + image, every example
$ scripts/smoke.sh --boot       # all three stages (needs KVM and sudo)
$ scripts/smoke.sh --boot cosmic
```

**Building without a compiler.** The machines most likely to run kuma are
image-based and ship no compiler, and layering one onto a bootc host to work
on the tool that builds bootc hosts is the wrong shape.
`scripts/Containerfile.dev` builds a container that has one. Cargo is not in
it: that comes from your home directory, so a build inside the container
shares `target/` and the registry cache with one outside it.

```console
$ podman build -t kuma-dev-gcc -f scripts/Containerfile.dev .
$ podman run --rm --userns=keep-id --security-opt label=disable \
    -v "$HOME:$HOME" -w "$PWD" -e "HOME=$HOME" kuma-dev-gcc \
    sh -c 'export PATH=$HOME/.cargo/bin:$PATH; cargo test'
```

CI runs formatting, tests, clippy at `-D warnings`, shellcheck, actionlint,
and the image stage on every committed example, desktops included. That
covers the compose and both desktop arms, so the build-time guards in them
run on a push. The boot stage stays local because its verdict comes from
booting the disk, which needs KVM and sudo. Run `--boot` locally before
pushing anything that changes what a machine does at runtime.

Two limits in that list worth knowing before you trust it. shellcheck reads
`scripts/smoke.sh` and nothing else, so the shell that images actually run
(the sync units and helpers, which live in Rust string literals in
`containerfile.rs`) is unchecked; run it by hand against the generated file
if you touch one. And the image stage proves an image builds, never that it
works, so anything whose behaviour appears at runtime has no gate on a push
at all.

actionlint is there because a workflow can be valid YAML and still be
rejected by Actions, which says so by running no job at all and leaving no
log to read.

A separate job runs `cargo audit` against the committed `Cargo.lock`, on every
push and again weekly, because a dependency becomes vulnerable when the
advisory lands rather than when someone next touches the tree.

**Cutting a release.** Bump `version` in `Cargo.toml`, refresh the lock,
rename the `Unreleased` section in `CHANGELOG.md` to the new version, commit
all three, then tag:

```console
$ cargo update -p kuma --offline   # Cargo.lock records kuma's own version
$ git tag -a v0.4.0 -m "kuma v0.4.0"
$ git push origin v0.4.0
```

`Cargo.lock` is not optional here. It carries the workspace member's version
too, so bumping only `Cargo.toml` leaves the lock disagreeing and every
`--locked` call fails, which is most of CI.

The tag and `Cargo.toml` have to agree as well. The release workflow checks
and fails rather than publishing a binary whose own `--version` contradicts
the release it sits in.

The changelog is checked twice, once where it can still be fixed cheaply. A
test fails locally when `Cargo.toml`'s version has no section, and the release
workflow fails on a tag whose section is missing, because it builds the release
notes out of that section. Leave a fresh empty `Unreleased` behind for the next
one: the rolling `latest` reads it, and entries are meant to land in the same
push as the change they describe.

What goes in it is what changes a machine, not what changed in the tree. Docs,
tests, and CI stay out. A release with nothing user-facing in it is a release
whose section says so in one line.

Push to `main` first and let it go green. A push that touches anything
other than documentation refreshes the rolling `latest` prerelease, and it
calls the release workflow rather than copying it, so main is a complete
rehearsal of every step a tag will run. A problem surfaces there while it
costs nothing, instead of once a tag already exists.

A release is one static `x86_64` binary, its checksum, and a Sigstore
bundle. The asset name carries no version on purpose: that is what keeps
the README's `releases/latest/download/` URL correct from one release to
the next, and a test pins the two together so a rename can't quietly break
the front door.

**Knowing which binary you have.** `kuma --version` reports the commit it
was built from and appends `-dirty` when that tree had uncommitted changes.
Worth checking whenever a change doesn't appear in the image: `kuma build`
runs whatever `kuma` is on `$PATH`, which is not necessarily the tree you
just edited.

**Booting a VM.** `kuma vm` builds a qcow2 via bootc-image-builder and boots
it in QEMU (it needs sudo; bootc-image-builder runs as root). Log in as your
declared `[user]`, or the always-present test user `kuma`/`kuma`
(`ssh -p 2222 kuma@localhost`; your ssh key is injected). Pass `--rebuild`
after rebuilding the image; kuma warns when the reused disk is older.

**Iterating without losing state.** `kuma vm --apply` streams the freshly
built image into the *running* VM and switches inside it. `/var` survives,
so flatpaks, brew, and homes don't re-download. It's also the real update
path, so `bootc rollback` inside the VM undoes it.

**Installer media.** `kuma iso` builds an Anaconda installer ISO
(`iso/bootiso/install.iso`), bootable in GNOME Boxes or `dd`'d to a USB
stick. Kuma-owned choices are preseeded; the rest is interactive. A declared
`[user]` rides into the installer, and `kuma iso` says so when it happens,
so build shareable media from a declaration without one.

**Inspecting an image.** It's a normal OCI image:
`podman run --rm -it localhost/kuma:latest bash`.

