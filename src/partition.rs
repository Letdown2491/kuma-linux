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

// Nothing calls this yet, and that is deliberate rather than an
// oversight. `kuma install` still hands partitioning to
// `bootc install to-disk`, so printing this layout in the plan would
// describe a disk kuma does not create. The layout is worth agreeing on
// before the code that writes it exists, because a partition table is
// the one thing here that cannot be revised afterwards, so it lands
// first and alone. The allow comes off in the commit that formats a
// disk with it.
#![allow(dead_code)]

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
    /// sgdisk type code.
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
            type_code: "ef00",
            purpose: "bootloader, read by the firmware",
        },
        Partition {
            label: "boot",
            size_mib: Some(BOOT_MIB),
            type_code: "8300",
            purpose: "kernels and initramfs, outside any encryption",
        },
        Partition {
            label: "root",
            size_mib: None,
            type_code: "8300",
            purpose: if encrypt { "LUKS, holding a btrfs root" } else { "btrfs root" },
        },
    ])
}

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
