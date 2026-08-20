# Kuma documentation

Start at [the project README](../README.md) for what kuma is and how to
install it. From there:

- **[Getting started](getting-started.md)** walks the whole path once:
  installing real hardware from the published media, then describing an image
  of your own, building it, and trying it in a VM. Start here if you want to
  use kuma.
- **[How kuma behaves](concepts.md)** explains the reasoning: what a build
  pins, what happens to changes you make by hand, why a file you edited keeps
  winning over the image, how a bad update rolls itself back, what a backup
  needs that a declaration deliberately will not hold, and what an install
  decides that a declaration cannot. Read this when something
  surprises you, or before trusting kuma with a machine.
- **[What a desktop contains](desktops.md)** lists what `desktop = "niri"` or
  `"cosmic"` installs that you never named, and why the odd-looking members
  are there.
- **[Glossary](glossary.md)** defines every term these documents use, one
  line each.
- **[For agents](agents.md)** describes the JSON surface for driving kuma
  from a program.

Two more live at the top of the repository: [SECURITY.md](../SECURITY.md) for
what a declaration trusts, how to verify a release, and what disk encryption
does and does not protect, and [CONTRIBUTING.md](../CONTRIBUTING.md) for
working on kuma itself.
