//! The partition layout `kuma install` creates.
//!
//! Separated from the code that writes it, and pure, because this is the
//! one decision in kuma that cannot be revised: a machine's partition
//! table is fixed at install and changing it means reinstalling. Every
//! choice here should be readable without running anything, and testable
//! without a disk.
//!
//! `bootc install to-disk` made these choices itself, and made different
//! ones: a 1 MiB BIOS boot partition, a 512 MiB ESP, and everything else
//! as root with `/boot` sitting inside it. That is a fine layout and it
//! forecloses encryption, because a LUKS root with `/boot` inside it
//! cannot be read by a bootloader that has not unlocked it yet. Owning
//! the layout is what buys the encrypted case, and the memory ceiling
//! besides: partitions kuma made are partitions kuma can mount before
//! pulling, so the image lands on the target disk rather than in RAM.

use anyhow::{bail, Result};

/// Fedora's own ESP size, and the one Anaconda gives a kuma machine
/// today. Big enough for several vendors' shim and grub builds, small
/// enough not to matter.
const ESP_MIB: u64 = 600;

/// `/boot`, outside the encryption.
///
/// Separate on every install, encrypted or not, so that turning
/// encryption on is not a different layout. It has to be outside a LUKS
/// root because GRUB reads the kernel before anything is unlocked, and
/// 2 GiB rather than Fedora's 1 GiB because an ostree machine keeps a
/// kernel per deployment and a full `/boot` is how an update fails at
/// the last moment.
const BOOT_MIB: u64 = 2048;

/// Below this there is no room for a system after `/boot` and the ESP,
/// and a person is better told that than left to find out when the
/// install runs out of space partway through writing.
const MIN_DISK_GIB: u64 = 16;

/// One partition, in the order it is created.
#[derive(Debug, PartialEq)]
pub struct Partition {
    /// GPT partition label, which is also how the installed system
    /// finds it: labels survive a disk being moved between machines,
    /// where a device name does not.
    pub label: &'static str,
    /// None means "the rest of the disk".
    pub size_mib: Option<u64>,
    /// sfdisk type alias, which it expands to the GPT type GUID.
    pub type_code: &'static str,
    /// What it is for, in words, for the plan a person reads.
    pub purpose: &'static str,
}

impl Partition {
    /// How the plan prints it: a size somebody can compare against the
    /// disk they are about to lose.
    pub fn size_text(&self, disk_mib: u64) -> String {
        match self.size_mib {
            Some(mib) if mib >= 1024 => format!("{:.0}G", mib as f64 / 1024.0),
            Some(mib) => format!("{mib}M"),
            None => {
                let rest = disk_mib.saturating_sub(ESP_MIB + BOOT_MIB);
                format!("{:.0}G", rest as f64 / 1024.0)
            }
        }
    }
}

/// The layout, for a disk of this size.
///
/// Three partitions, always the same three. `encrypt` changes what goes
/// *inside* the third one, not whether it exists, so that a machine
/// installed with encryption and one without differ in one place rather
/// than in their shape.
pub fn plan(disk_bytes: u64, encrypt: bool) -> Result<Vec<Partition>> {
    let disk_mib = disk_bytes / (1024 * 1024);
    if disk_mib < MIN_DISK_GIB * 1024 {
        bail!(
            "{:.1}G is too small to install onto: {}M of ESP and {}M of /boot leave \
             no room for a system. {MIN_DISK_GIB}G is the minimum.",
            disk_bytes as f64 / 1e9,
            ESP_MIB,
            BOOT_MIB
        );
    }
    Ok(vec![
        Partition {
            label: "EFI-SYSTEM",
            size_mib: Some(ESP_MIB),
            type_code: "uefi",
            purpose: "bootloader, read by the firmware",
        },
        Partition {
            label: "boot",
            size_mib: Some(BOOT_MIB),
            type_code: "linux",
            purpose: "kernels and initramfs, outside any encryption",
        },
        Partition {
            label: "root",
            size_mib: None,
            type_code: "linux",
            purpose: if encrypt { "LUKS, holding a btrfs root" } else { "btrfs root" },
        },
    ])
}

/// The subvolume the installed root lives in.
///
/// Named in three places that have to agree: the subvolume created at
/// install, the mount the installer does, and the kernel argument the
/// machine boots with. One constant so they cannot drift.
pub const ROOT_SUBVOL: &str = "root";

/// Where the container store goes while installing. Deleted afterwards.
pub const STORE_SUBVOL: &str = "store";

/// The tag the account-carrying layer is built under.
///
/// Never leaves the target: it is built into a store that lives on a
/// subvolume the cleanup trap deletes, so the layer holding a password
/// hash cannot outlive the install even when the install fails.
pub const INSTALL_TAG: &str = "localhost/kuma-install:latest";

/// Everything the install script calls that a machine might not have,
/// and the package that carries it.
///
/// Checked before the interview rather than discovered partway through,
/// because the alternative is what `sgdisk` did: the disk gets wiped, a
/// password gets typed, and the script stops with `command not found`
/// and exit 127. Every one of these is on a kuma image already; the list
/// exists because the installer also runs on machines that are not kuma
/// ones, and because a missing tool should name its package.
pub const REQUIRED_TOOLS: &[(&str, &str)] = &[
    ("losetup", "util-linux"),
    ("wipefs", "util-linux"),
    ("sfdisk", "util-linux"),
    ("udevadm", "systemd-udev"),
    ("mkfs.vfat", "dosfstools"),
    ("mkfs.ext4", "e2fsprogs"),
    ("mkfs.btrfs", "btrfs-progs"),
    ("btrfs", "btrfs-progs"),
    ("podman", "podman"),
];

/// Partition and format a disk, then mount it ready for bootc.
///
/// A script rather than a sequence of Rust calls, for the same reason
/// kuma's boot-time convergers are scripts: it can be extracted,
/// shellchecked, and read as one thing. Every command in it is
/// destructive, so it is generated from the plan rather than written
/// twice, and takes its device and mount point as arguments so a test
/// can point it at a loop device instead of somebody's disk.
pub fn format_script(plan: &[Partition]) -> String {
    let mut out = String::from(
        "#!/usr/bin/bash\n\
         # Generated by kuma from partition::plan. Every line here destroys\n\
         # something; the caller has already refused a mounted disk and\n\
         # printed this layout for a person to agree with.\n\
         set -euo pipefail\n\
         dev=${1:?device to partition}\n\
         mnt=${2:?where to mount the result}\n\
         # The partition nodes are passed in rather than derived here:\n\
         # a file target is reached through a loop device that does not\n\
         # exist until the caller attaches it, so the name cannot be\n\
         # known before then.\n\
         fsmnt=${3:?where to mount the filesystem top level}\n\
         esp=${4:?esp device}\n\
         boot=${5:?boot device}\n\
         root=${6:?root device}\n\n\
         # A partition table with anything left of an old one is how a\n\
         # stale signature survives to confuse a later probe.\n\
         wipefs --all --force \"$dev\" >/dev/null\n\n\
         # One call, not one per partition, and sfdisk rather than the\n\
         # gdisk family: sfdisk is util-linux, which every bootc machine\n\
         # already has, and gdisk is a package a kuma image does not\n\
         # ship, so the installer could not run on the media built to\n\
         # run it. Writing the table in one call also means a failure\n\
         # leaves the old table rather than half of a new one.\n\
         sfdisk --wipe always --quiet \"$dev\" <<'PARTITIONS'\n\
         label: gpt\n",
    );
    for part in plan {
        let size = match part.size_mib {
            Some(mib) => format!("size={mib}MiB, "),
            // The last one takes what is left, which sfdisk does by
            // being told no size at all.
            None => String::new(),
        };
        out.push_str(&format!("{size}type={}, name=\"{}\"\n", part.type_code, part.label));
    }
    out.push_str(
        "PARTITIONS\n\n\
         # The kernel learns about the new table asynchronously, and\n\
         # mkfs on a node that does not exist yet fails in a way that\n\
         # reads like a bad disk.\n\
         udevadm settle\n\n",
    );
    out.push_str(&format!(
        "mkfs.vfat -F32 -n EFI-SYSTEM \"$esp\" >/dev/null\n\
         mkfs.ext4 -q -F -L boot \"$boot\"\n\
         mkfs.btrfs -q -f -L root \"$root\"\n\n\
         # Two subvolumes, and the second one is why this is btrfs work\n\
         # rather than a plain mkfs. bootc install to-filesystem requires\n\
         # the root it is given to be empty, and --replace wipe would\n\
         # destroy anything left there, so the container store cannot sit\n\
         # inside the target. It sits beside it: `root` is what gets\n\
         # installed and looks empty, `store` holds the image being\n\
         # pulled, on the target disk rather than in the RAM a live\n\
         # session would otherwise have to find. It is deleted when the\n\
         # install finishes.\n\
         mkdir -p \"$fsmnt\"\n\
         mount \"$root\" \"$fsmnt\"\n\
         btrfs subvolume create \"$fsmnt/{ROOT_SUBVOL}\" >/dev/null\n\
         btrfs subvolume create \"$fsmnt/{STORE_SUBVOL}\" >/dev/null\n\n\
         # Mounted in the order bootc expects to find them: the root\n\
         # first, then what hangs off it. subvol=root, which is also what\n\
         # the installed machine will be told to mount, so the karg and\n\
         # this line have to say the same thing.\n\
         mkdir -p \"$mnt\"\n\
         mount -o subvol={ROOT_SUBVOL} \"$root\" \"$mnt\"\n\
         mkdir -p \"$mnt/boot\"\n\
         mount \"$boot\" \"$mnt/boot\"\n\
         mkdir -p \"$mnt/boot/efi\"\n\
         mount \"$esp\" \"$mnt/boot/efi\"\n"
    ));
    out
}

/// The whole install, as one privileged script.
///
/// One script rather than a sequence of calls from Rust, because the
/// failure path is what matters: half a partition table, a mounted
/// target and an attached loop device are a worse outcome than a clean
/// stop, and a trap unwinds all of it wherever the failure happened.
/// Rust would need the same unwinding spread across every `?`.
///
/// Built by substitution rather than `format!`: the text is shell, shell
/// is full of braces, and escaping every one of them to satisfy a
/// formatter makes the script unreadable in the place it most needs
/// reading.
pub fn install_script(plan: &[Partition]) -> String {
    // The formatting half, minus its shebang and argument preamble,
    // which this script has already done for itself.
    let format_body: String = format_script(plan)
        .lines()
        .skip_while(|l| !l.starts_with("wipefs"))
        .collect::<Vec<_>>()
        .join("\n");
    INSTALL_TEMPLATE
        .replace("@FORMAT@", &format_body)
        .replace("@STORE@", STORE_SUBVOL)
        .replace("@TAG@", INSTALL_TAG)
        .replace("@SUBVOL@", ROOT_SUBVOL)
}

const INSTALL_TEMPLATE: &str = r##"#!/usr/bin/bash
# Generated by kuma. Runs as root, and every line past the partitioning
# destroys something.
set -euo pipefail

target=${1:?disk or disk image to install onto}
ctx=${2:?build context holding the Containerfile and the answers}
updates=${3:?where the installed machine fetches updates from}

# Fixed paths, not mktemp. These have to be reachable by that name from
# inside a container as well as out here, so a name chosen at random by
# one of them is a name the other has to be told; /run is tmpfs, root
# only, and gone at reboot whatever this script manages to leave behind.
mnt=/run/kuma-target
fsmnt=/run/kuma-targetfs
mkdir -p "$mnt" "$fsmnt"
loop=""
conf=""

# Unwinds wherever it stopped. A failure that leaves the target mounted
# and a loop device attached makes the next attempt fail for a different
# reason than the first one did, which is how an afternoon disappears.
cleanup() {
    umount -R "$mnt" 2>/dev/null || true
    # Not tidiness: that subvolume holds the image layer carrying the
    # account's password hash, and it must not survive the install.
    btrfs subvolume delete "$fsmnt/@STORE@" >/dev/null 2>&1 || true
    umount "$fsmnt" 2>/dev/null || true
    rm -f "${conf:-}" 2>/dev/null || true
    rmdir "$mnt" "$fsmnt" 2>/dev/null || true
    if [ -n "$loop" ]; then losetup -d "$loop" 2>/dev/null || true; fi
}
trap cleanup EXIT

# A file is a disk image, reached through a loop device. -P so the kernel
# creates nodes for the partitions this is about to write.
if [ -f "$target" ]; then
    loop=$(losetup -fP --show "$target")
    dev=$loop
else
    dev=$target
fi

case "$dev" in
    *[0-9]) esp="${dev}p1"; boot="${dev}p2"; root="${dev}p3" ;;
    *)      esp="${dev}1";  boot="${dev}2";  root="${dev}3" ;;
esac

@FORMAT@

# The store goes on the target, which is the whole reason for the second
# subvolume: a live session has nowhere else to put an image this size
# except RAM, and half of RAM is not enough on an ordinary laptop.
#
# additionalimagestore so a locally built image can be installed without
# copying it here first. That store is read-only to podman, so a pull
# still lands on the target.
#
# The driver is named rather than detected, because the target is btrfs
# and podman's btrfs driver makes a subvolume per layer. Deleting the
# store afterwards would then fail on a subvolume that is not empty, and
# the layer holding the password hash would stay on the installed disk.
store="$fsmnt/@STORE@"
mkdir -p /var/lib/containers/storage
podman --root "$store" --runroot /run/kuma-install --storage-driver overlay \
    --storage-opt additionalimagestore=/var/lib/containers/storage \
    build -q -t @TAG@ -f "$ctx/Containerfile" "$ctx" >/dev/null

# bootc is handed the target path and opens it, and if the bind did not
# arrive it says so only after the disk has been partitioned, formatted
# and written to: `Opening target root directory ...: No such file or
# directory`, with nothing to say the mount was the problem. Asked here
# instead, of the same image that is about to do the install, where it
# costs a second and names what is actually wrong.
if ! podman --root "$store" --runroot /run/kuma-install --storage-driver overlay \
    --storage-opt additionalimagestore=/var/lib/containers/storage \
    run --rm --privileged --security-opt label=disable \
    -v "$mnt:$mnt" @TAG@ test -d "$mnt"; then
    echo "kuma: $mnt is mounted out here but not visible inside a container." >&2
    echo "kuma: installing would have failed after the disk was formatted." >&2
    exit 1
fi

# bootc does not merely copy the filesystem of the container it runs in.
# It reads /run/.containerenv, takes the image ID podman recorded there,
# and asks container storage what that ID is, so it can record what the
# machine was installed from. Asking means the default store as seen
# from inside the container, which this store is not, and the failure
# reads `no such object` followed by a digest, after the disk has
# already been partitioned and formatted.
#
# --source-imgref skips that discovery and names the source outright.
# The store is reachable inside the container because $fsmnt is mounted
# at the same path, so one config describes it for both: the host's
# store is an additional image store, which is what lets an image built
# here rather than pulled resolve its base layers.
conf=$(mktemp)
cat > "$conf" <<STORAGE
[storage]
driver = "overlay"
graphroot = "$store"
runroot = "/run/kuma-install"
[storage.options]
additionalimagestores = ["/var/lib/kuma-host-store/storage"]
STORAGE

# bootc copies the filesystem of the container it runs in, so it runs in
# the derived image and is handed the target already mounted. The root is
# named by LABEL rather than by device, because a disk that moves between
# machines keeps its labels and loses its device names.
podman --root "$store" --runroot /run/kuma-install --storage-driver overlay \
    --storage-opt additionalimagestore=/var/lib/containers/storage \
    run --rm --privileged --pid=host --security-opt label=disable \
    -v /dev:/dev -v "$mnt:$mnt" -v "$fsmnt:$fsmnt" \
    -v /var/lib/containers:/var/lib/kuma-host-store \
    -v "$conf:/etc/containers/storage.conf:ro" \
    @TAG@ \
    bootc install to-filesystem \
        --source-imgref "containers-storage:@TAG@" \
        --root-mount-spec "LABEL=root" \
        --boot-mount-spec "LABEL=boot" \
        --karg "rootflags=subvol=@SUBVOL@" \
        --target-imgref "$updates" \
        "$mnt"
"##;

#[cfg(test)]
mod tests {
    use super::*;

    /// Three partitions whatever else is decided, so that turning
    /// encryption on is not a different disk shape.
    #[test]
    fn the_layout_does_not_change_with_encryption() {
        let plain = plan(40 * 1_000_000_000, false).unwrap();
        let crypt = plan(40 * 1_000_000_000, true).unwrap();
        assert_eq!(plain.len(), 3);
        assert_eq!(
            plain.iter().map(|p| p.label).collect::<Vec<_>>(),
            crypt.iter().map(|p| p.label).collect::<Vec<_>>()
        );
        // Only what lives inside the root partition differs.
        assert_ne!(plain[2].purpose, crypt[2].purpose);
        assert_eq!(plain[2].type_code, crypt[2].type_code);
    }

    /// /boot is its own partition even unencrypted. GRUB reads a kernel
    /// before anything is unlocked, so a LUKS root with /boot inside it
    /// cannot boot, and having the layout depend on that choice would
    /// mean encryption could never be turned on without reinstalling
    /// into a different shape.
    #[test]
    fn boot_is_always_outside_the_root() {
        let p = plan(40 * 1_000_000_000, false).unwrap();
        assert_eq!(p[1].label, "boot");
        assert!(p[1].size_mib.is_some(), "/boot is sized, not the remainder");
        assert_eq!(p[2].size_mib, None, "root takes what is left");
    }

    /// Refused rather than attempted. Running out of room partway
    /// through writing a partition table is a worse way to learn this
    /// than being told before anything is touched.
    #[test]
    fn a_disk_too_small_is_refused_with_the_arithmetic() {
        let err = plan(8 * 1_000_000_000, false).unwrap_err().to_string();
        assert!(err.contains("too small"));
        assert!(err.contains("16G"), "says what would be enough");
        assert!(plan(16 * 1024 * 1024 * 1024, false).is_ok());
    }

    /// /dev/sda numbers its partitions sda1; /dev/nvme0n1 numbers them
    /// nvme0n1p1. Loop devices follow the nvme rule, so a test against a
    /// disk image exercises the same branch a real NVMe does rather than
    /// the easier one.
    ///
    /// Checked by running the script's own case statement rather than by
    /// reimplementing it: the rule has to be applied after the loop
    /// device is attached, so shell is where it lives, and a Rust copy
    /// of it would be a second implementation that could agree with the
    /// test while disagreeing with the script.
    #[test]
    fn partition_nodes_follow_the_rule_for_each_kind_of_disk() {
        let case = INSTALL_TEMPLATE
            .split_once("case \"$dev\" in")
            .map(|(_, rest)| rest.split_once("esac").unwrap().0)
            .map(|body| format!("case \"$dev\" in{body}esac\necho \"$esp $boot $root\""))
            .unwrap();
        for (dev, want) in [
            ("/dev/sda", "/dev/sda1 /dev/sda2 /dev/sda3"),
            ("/dev/vda", "/dev/vda1 /dev/vda2 /dev/vda3"),
            ("/dev/nvme0n1", "/dev/nvme0n1p1 /dev/nvme0n1p2 /dev/nvme0n1p3"),
            ("/dev/loop0", "/dev/loop0p1 /dev/loop0p2 /dev/loop0p3"),
            ("/dev/mmcblk0", "/dev/mmcblk0p1 /dev/mmcblk0p2 /dev/mmcblk0p3"),
        ] {
            let out = std::process::Command::new("bash")
                .args(["-c", &format!("dev={dev}\n{case}")])
                .output()
                .expect("bash");
            assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), want, "for {dev}");
        }
    }

    /// A tool the script calls and the list does not name is one that
    /// fails after the disk is gone. This is the check that made itself
    /// necessary: `sgdisk` was called by a script running on media that
    /// had no gdisk, and nothing said so until the table was already
    /// wiped.
    #[test]
    fn every_tool_the_script_calls_is_declared() {
        let script = install_script(&plan(40 * 1024 * 1024 * 1024, false).unwrap());
        for (tool, _) in REQUIRED_TOOLS {
            assert!(script.contains(tool), "{tool} is declared but never called");
        }
        // The other direction cannot be checked by parsing shell without
        // writing a shell parser, so it is checked against the tools
        // that were actually reached for and are not there: gdisk and
        // parted are packages a kuma image does not ship, and reaching
        // for either again would fail the same way.
        for absent in ["sgdisk", "partprobe", "parted", "cgdisk"] {
            assert!(!script.contains(absent), "{absent} is not on a kuma machine");
        }
    }

    /// The script destroys everything it touches, so what it contains is
    /// worth pinning: the wipe before the table, a partition line per
    /// entry in the plan, and the settle without which mkfs runs against
    /// a node the kernel has not created yet.
    #[test]
    fn the_writer_matches_the_plan_it_was_given() {
        let p = plan(40 * 1024 * 1024 * 1024, false).unwrap();
        let script = format_script(&p);
        assert!(script.contains("wipefs --all"));
        // One table written in one call, so a failure leaves the old one
        // rather than half of a new one.
        assert_eq!(script.matches("sfdisk --wipe always").count(), 1);
        assert!(script.contains("label: gpt"));
        assert!(script.contains(r#"size=600MiB, type=uefi, name="EFI-SYSTEM""#));
        assert!(script.contains(r#"size=2048MiB, type=linux, name="boot""#));
        assert!(script.contains(r#"type=linux, name="root""#));
        assert!(
            !script.contains(r#"size=0MiB, type=linux, name="root""#),
            "root takes the remainder, which sfdisk spells as no size at all"
        );
        // The heredoc terminator has to sit at column 0 or the script is
        // one unterminated string.
        assert!(script.contains("\nPARTITIONS\n"));
        assert!(script.contains("udevadm settle"));
        // Mounted root first, then what hangs off it.
        let root_at = script.find(r#"mount "$root""#).unwrap();
        let boot_at = script.find(r#"mount "$boot""#).unwrap();
        let esp_at = script.find(r#"mount "$esp""#).unwrap();
        assert!(root_at < boot_at && boot_at < esp_at);
        // Nothing derives partition names in shell.
        assert!(!script.contains("kuma_part"));
    }

    /// Not an assertion: writes the generated script out so it can be
    /// shellchecked by hand, since CI only checks scripts/ and every
    /// script kuma embeds has shipped unchecked before.
    #[test]
    fn dump_the_script_for_shellcheck() {
        if let Ok(path) = std::env::var("KUMA_DUMP_SCRIPT") {
            let p = plan(40 * 1024 * 1024 * 1024, false).unwrap();
            std::fs::write(path, install_script(&p)).unwrap();
        }
    }

    /// The subvolume is named in three places that must agree: created
    /// by the script, mounted by the script, and named in the kernel
    /// argument the installed machine boots with. If they drift, the
    /// install succeeds and the machine cannot find its root.
    #[test]
    fn the_root_subvolume_is_named_once() {
        let script = format_script(&plan(40 * 1024 * 1024 * 1024, false).unwrap());
        assert!(script.contains(&format!("subvolume create \"$fsmnt/{ROOT_SUBVOL}\"")));
        assert!(script.contains(&format!("mount -o subvol={ROOT_SUBVOL}")));
        // The store is a sibling, never inside the target: bootc requires
        // an empty root and --replace wipe would destroy it.
        assert!(script.contains(&format!("subvolume create \"$fsmnt/{STORE_SUBVOL}\"")));
        assert_ne!(ROOT_SUBVOL, STORE_SUBVOL);
    }

    /// The script that actually runs: it has to carry the formatting
    /// steps, unwind on failure, and name the root the same way twice.
    #[test]
    fn the_install_script_is_whole() {
        let script = install_script(&plan(40 * 1024 * 1024 * 1024, false).unwrap());
        // The formatting half is carried, not re-described.
        assert!(script.contains("wipefs --all"));
        assert!(script.contains("mkfs.btrfs"));
        assert_eq!(script.matches("sfdisk --wipe always").count(), 1);
        // Nothing of the sub-script's own preamble came with it.
        assert_eq!(script.matches("#!/usr/bin/bash").count(), 1);
        assert_eq!(script.matches("set -euo pipefail").count(), 1);
        // Cleanup runs wherever it stopped.
        assert!(script.contains("trap cleanup EXIT"));
        assert!(script.contains("losetup -d"));
        // The root is named the same way in the mount and the karg.
        assert!(script.contains(&format!("--karg \"rootflags=subvol={ROOT_SUBVOL}\"")));
        assert!(script.contains(&format!("mount -o subvol={ROOT_SUBVOL}")));
        // The store is on the target, not in RAM, and not inside it.
        assert!(script.contains(&format!("store=\"$fsmnt/{STORE_SUBVOL}\"")));
        assert!(script.contains("--root \"$store\""));
        // No placeholder survived substitution.
        assert!(!script.contains('@'), "an @PLACEHOLDER@ was left unreplaced");
        // The store holds a layer with a password hash in it, so the
        // driver is pinned: podman's btrfs driver would make a
        // subvolume per layer and the cleanup delete would fail on a
        // non-empty one, leaving the hash on the installed disk.
        assert!(script.contains("--storage-driver overlay"));
    }

    /// The sizes a person compares against the disk they are losing.
    #[test]
    fn sizes_read_the_way_a_person_would_write_them() {
        let disk_mib = 40 * 1024;
        let p = plan(40 * 1024 * 1024 * 1024, false).unwrap();
        assert_eq!(p[0].size_text(disk_mib), "600M");
        assert_eq!(p[1].size_text(disk_mib), "2G");
        // The remainder is what is left after the other two, not the
        // whole disk, which is the number somebody is actually getting.
        assert_eq!(p[2].size_text(disk_mib), "37G");
    }
}
