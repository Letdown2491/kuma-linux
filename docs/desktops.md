# What a desktop contains

Setting `desktop = "niri"` or `desktop = "cosmic"` installs a set of packages
you did not name. This page says what they are and why.

A desktop is more than the thing you look at: something has to draw windows,
show a login screen, play sound, find printers, ask for your password when an
application needs root, and supply fonts. Naming a desktop is how you get all
of that in one word.

For the exact list any declaration produces, run `kuma generate` and read the
`dnf install` line. That is the authority; this page explains it.

## Three layers, two owners

An image is built in layers, and they are not all curated by the same hand:

1. **Fedora's minimal bootc core.** Kernel, systemd, bootc, dnf. Fedora
   maintains it; kuma includes it unmodified.
2. **kuma's base.** Networking, firmware, and the handful of things any real
   machine needs: `shadow-utils`, `sudo`, `chrony`, `openssh-server`,
   `passwd`, `cryptsetup`, `fwupd`. See
   [where the base system comes from](concepts.md#where-the-base-system-comes-from).
3. **The desktop set.** Everything below.

## The line between a desktop and your declaration

A curated desktop holds the parts that have to be present for a session to
function at all. Applications are not in it. A calculator, an office suite, a
media player, or a store belongs in `packages.flatpak` in your declaration,
even when it would be convenient to bake one in.

The reason is reversibility rather than taste. A line in your declaration is a
suggestion: delete it and the next convergence uninstalls it. A package in a
desktop set is a decree, because there is no way to remove one (see
[what you can change](#what-you-can-change) below). Being opinionated in an
example declaration costs nothing. Being opinionated in the desktop set costs
you a fork.

## niri

Hand-assembled, because niri is a window manager rather than a desktop: it
requires nothing beyond itself, so every part of a working session is named
explicitly.

| | |
|---|---|
| Session | `niri`, `xwayland-satellite`, `greetd`, `tuigreet` |
| Shell | `waybar`, `fuzzel`, `mako`, `wob`, `swaybg`, `swayidle`, `swaylock` |
| Terminal and files | `kitty`, `thunar` (+ archive plugin), `file-roller`, `gvfs`, `udiskie`, `7zip`, `unar` |
| Portals | `xdg-desktop-portal-gtk`, `xdg-desktop-portal-gnome` |
| Audio | `pipewire`, `pipewire-pulseaudio`, `wireplumber`, `pavucontrol` |
| Graphics | `mesa-dri-drivers`, `mesa-vulkan-drivers`, `vulkan-loader` |
| Hardware | `NetworkManager-wifi`, `wpa_supplicant`, `bluez`, `blueman`, `brightnessctl`, `power-profiles-daemon` |
| Printing and discovery | `cups`, `system-config-printer`, `avahi`, `nss-mdns` |
| Screen and clipboard | `grim`, `slurp`, `swappy`, `wf-recorder`, `wl-clipboard`, `cliphist` |
| Session glue | `polkit`, `mate-polkit`, `dconf`, `gnome-keyring`, `xsettingsd`, `xdg-user-dirs`, `firewalld`, `flatpak` |
| Fonts | sans, mono, emoji, CJK, and Font Awesome (free and brands) |

Graphical settings for wifi, bluetooth, audio, and printers are included on
purpose. Those are machine state, not system definition: the declaration
describes what a machine is, and picking a network is not that.

## COSMIC

Much shorter, because COSMIC curates itself. `cosmic-session` hard-requires
the coherent desktop (compositor, panel, applets, settings, files, terminal,
notifications, OSD, screenshot, portal, fonts), so kuma names the session plus
the hardware enablement a desktop lives on, and little else.

The additions worth knowing: `cosmic-edit`, because the default dock pins it
and the session does not require it, so without it the pin is dead;
`pipewire`, because the session requires the client library but nothing pulls
the daemon; and `udisks2`, because `cosmic-files` mounts removable media
through it directly.

`cosmic-store` is deliberately absent. A store is an application, so which one
a machine gets is the declaration's call.

## Why these are here

Most of a desktop set is unsurprising. These are not, and each one is a
failure someone had to diagnose:

- **`gnome-keyring-pam`** unlocks the login keyring at login. It is a separate
  subpackage that nothing depends on, and the greeter's PAM lines are `-`
  prefixed, so a missing module is skipped in total silence. Without it, login
  succeeds, nothing is logged, and every keyring-using app prompts on launch
  forever.
- **`nss-mdns`** is what makes `.local` names and driverless printer discovery
  actually resolve. `avahi` alone announces without resolving.
- **`zram-generator-defaults`** carries the config that activates zram. The
  base ships the generator without it, so the desktop would have zero swap and
  the OOM killer would take windows under memory pressure.
- **`glibc-langpack-en`** provides real locale data. The base ships
  `glibc-minimal-langpack`, so `en_US.UTF-8` fails to resolve and waybar's
  clock disables itself.
- **`mesa-vulkan-drivers`** and **`vulkan-loader`**: OpenGL drivers alone
  strand every Vulkan application on software rendering.
- **`fontawesome-6-brands-fonts`** is a second font package because the
  bluetooth glyph waybar uses lives in the Brands face, not Free.
- **`google-noto-sans-cjk-vf-fonts`**, because the default sans is latin-only
  and CJK pages render as tofu.
- **`avahi`** is named rather than assumed. It used to arrive with
  fedora-bootc by luck, and kuma's composed base does not carry it.

## What you can change

**You can add.** `packages.rpm` layers anything from Fedora's repos on top of
the desktop set.

**You cannot subtract.** There is no `rpm_exclude` and no per-desktop opt-out,
so a package in a desktop set is in your image. If you want a desktop without
Thunar, the only route today is a fork.

That is a real limit rather than an oversight, and it is why the boundary above
is drawn where it is: everything kuma cannot let you remove is something a
session needs in order to work, and everything else is left to your
declaration where deleting a line is enough.
