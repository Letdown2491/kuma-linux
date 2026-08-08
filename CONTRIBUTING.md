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

CI runs formatting, tests, clippy at `-D warnings`, shellcheck, and the image
stage on the minimal example: a desktop image doesn't fit a hosted runner's
disk, and the boot stage needs KVM. Run `--boot` locally before pushing
anything that touches image contents.

A separate job runs `cargo audit` against the committed `Cargo.lock`, on every
push and again weekly, because a dependency becomes vulnerable when the
advisory lands rather than when someone next touches the tree.

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

