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

A curated desktop holds session infrastructure; applications belong in
`packages.flatpak`. The reason is reversibility rather than taste, and it is
argued once in
[why a desktop installs things you did not name](concepts.md#why-a-desktop-installs-things-you-did-not-name)
rather than twice here. What that rule costs in practice is
[what you can change](#what-you-can-change), below.

## niri

Hand-assembled, because niri is a window manager rather than a desktop: it
requires nothing beyond itself, so every part of a working session is named
explicitly. Most of that session is now one program. Noctalia draws the bar,
notifications, wallpaper, on-screen displays, the lock screen and a control
centre, where kuma previously assembled seven separate tools that agreed on
colour and on nothing else.

| | |
|---|---|
| Session | `niri`, `xwayland-satellite`, `greetd`, `tuigreet` |
| Shell | `noctalia` (bar, notifications, wallpaper, OSDs, idle, lock, control centre) |
| Terminal and files | `kitty`, `thunar` (+ archive plugin), `file-roller`, `gvfs`, `udiskie`, `7zip`, `unar` |
| Portals | `xdg-desktop-portal-gtk`, `xdg-desktop-portal-gnome` |
| Audio | `pipewire`, `pipewire-pulseaudio`, `wireplumber`, `pavucontrol` |
| Graphics | `mesa-dri-drivers`, `mesa-vulkan-drivers`, `vulkan-loader` |
| Hardware | `NetworkManager-wifi`, `NetworkManager-tui`, `wpa_supplicant`, `bluez`, `blueman`, `brightnessctl`, `power-profiles-daemon` |
| Printing and discovery | `cups`, `system-config-printer`, `avahi`, `nss-mdns` |
| Screen and clipboard | `grim`, `slurp`, `swappy`, `wf-recorder`, `wl-clipboard` |
| Session glue | `polkit`, `mate-polkit`, `dconf`, `gnome-keyring`, `xsettingsd`, `xdg-user-dirs`, `firewalld`, `flatpak`, `desktop-file-utils` |
| Fonts and icons | sans, mono, emoji, CJK, Font Awesome (free and brands), `adwaita-icon-theme`, `adw-gtk3-theme` |

The control centre owns the everyday cases: wifi, bluetooth, audio,
brightness, night light, and the power button in its header opens lock, log
out, suspend, reboot and shut down. The separate settings tools stay for what
it does not reach — `nm-connection-editor` for a VPN or a static route,
`pavucontrol` for per-application routing, `system-config-printer` for
printers. All of it is
machine state rather than system definition: the declaration describes what a
machine is, and picking a network is not that.

**The desktop's own look comes from the image.** Kuma bakes a noctalia config
— bar layout, fonts, wallpaper, and the idle lock and night light that
noctalia ships turned off — and the shell reads it from there. Changing
anything from the desktop's own settings writes
`~/.local/state/noctalia/settings.toml`, which wins over the image. That file
is yours and the image will not overwrite it. Kuma does not read it either, so
`kuma diff` will not mention it: `noctalia config export merged` is what shows
which settings are actually in effect.

**The terminal and GTK3 applications follow the shell's palette.** Noctalia
derives that palette from the wallpaper by default, and whatever it is showing,
the shell renders it into `~/.config/kitty/themes/noctalia.conf` and
`~/.config/gtk-3.0/noctalia.css` whenever it changes and again at login. Point
the shell at one of its built-in palettes instead and the terminal, thunar,
pavucontrol and the rest move with it. The image ships `adw-gtk3-theme` for
this: stock Adwaita GTK3 ignores the colour names a palette can set.

kitty's sixteen ANSI colours are the exception, and stay fixed in
`/etc/xdg/kitty/kitty.conf`. The palette maps every ANSI slot into its own hue
family, and a terminal whose green is blue makes a passing test look like a
failing one. To keep the image's fixed colours instead, delete
`~/.config/kitty/themes/noctalia.conf` and the include line in
`~/.config/kitty/kitty.conf`.

GTK4 applications do not follow, which on a kuma machine mostly means
flatpaks. libadwaita ignores a user stylesheet that redefines its palette, so
they keep their own dark theme.

The bar carries state and little else, so the two panels that are not state
are on keys: `Mod+Ctrl+V` for clipboard history, `Mod+Ctrl+W` for the
wallpaper picker. `Mod+Shift+/` lists every bind the session has.

`Mod+D` opens the shell's launcher. Your applications are in it, and so are
kuma's own verbs: edit the declaration, show drift, system health, check for
updates, rebuild, roll back, snapshots. Those arrive as ordinary desktop
entries rather than anything the shell knows about, so they are on the COSMIC
desktop too. See [kuma in your launcher](concepts.md#kuma-in-your-launcher).

## COSMIC

COSMIC is experimental. It is built on every push like every other example, so
it compiles and its build-time checks run, and it does boot. What it does not
get is the verification niri gets: the checks that install kuma to a disk and
then boot it and interrogate the running machine are run against niri on every
change, and against COSMIC when someone remembers. Treat it as the second
desktop in the order it is verified, not only in the order it is listed.

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
  the OOM killer would take windows under memory pressure. It is swap *in
  memory*, so it cannot hold a copy of memory: hibernating needs a file on a
  disk, which is what `kuma hibernate` makes.
- **`glibc-langpack-en`** provides real locale data. The base ships
  `glibc-minimal-langpack`, so `en_US.UTF-8` fails to resolve and anything
  formatting a date, a time or a number falls back to C.
- **`mesa-vulkan-drivers`** and **`vulkan-loader`**: OpenGL drivers alone
  strand every Vulkan application on software rendering.
- **`fontawesome-fonts-all`** pulls both Font Awesome faces. The shell bundles
  its own icon font, but flatpaks and GTK applications still reach for these
  glyphs and render tofu without them, and the brands live in a different face
  from the rest. It is named instead of the two faces directly because their
  package names carry the major version, which changes under you; this one
  does not, and it adds no files of its own.
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
