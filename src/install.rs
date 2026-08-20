//! Installing kuma onto a disk.
//!
//! The account is the whole difficulty. A published image declares no
//! `[user]`, because the image is shared and the person is not, so a
//! machine installed from one has no account and no root password and no
//! way in. Anaconda's create-a-user screen used to cover that, and live
//! media has no Anaconda.
//!
//! So the installer's real output is a declaration. It asks, writes
//! `/var/lib/kuma/user` on the target, and `kuma-user-sync` creates the
//! account at first boot exactly as it does for a declared one. The
//! installer does not create users; it writes down what the machine
//! should converge to, which is the same thing kuma does everywhere else.
//!
//! Getting that file onto the target needs no post-install mounting.
//! `bootc install` copies the filesystem of the container it runs inside,
//! so kuma derives a one-layer image carrying the file and installs from
//! that, while `--target-imgref` records the *published* image as what
//! the machine fetches for subsequent updates. The installed system
//! therefore has an account and still tracks the public tag.
//!
//! Scope is deliberately whole-disk: kuma owns the partitioning (see
//! `partition`), but not where the partitions go. A custom layout is a
//! separate decision and a later one.
//!
//! Encryption is offered rather than assumed. It is asked for, not
//! declared, because it is a property of a disk and not of an image: the
//! same declaration installed onto two machines can encrypt one and not
//! the other, and no image can carry the answer. It is also the one
//! answer here that cannot be revised without reinstalling, which is why
//! it is asked before anything is written rather than defaulted either
//! way.

use anyhow::{bail, Context, Result};

/// What was asked for, before any of it is checked.
///
/// A struct rather than eight positional parameters: this is the one
/// verb here that cannot be undone, and a call whose arguments are told
/// apart by position is a poor place to get one wrong.
pub struct Request {
    pub image: String,
    /// What a bare `kuma install` resolves to here, which is the
    /// published image on a machine and the media's own image on
    /// installer media that recorded a pullable one. Carried so the
    /// command a dry run prints names `--image` when, and only when,
    /// running it without the flag would install something else.
    pub default_image: String,
    /// What the installed machine fetches updates from, when that is not
    /// the image being installed. Installing a locally built image and
    /// tracking the published tag is the case this exists for.
    pub update_from: Option<String>,
    pub user: Option<String>,
    pub groups: Vec<String>,
    pub hostname: Option<String>,
    pub shell: Option<String>,
    /// Asked for on a terminal when this is false, so that the flag is a
    /// way to answer the question early rather than the only way to
    /// answer it at all.
    pub encrypt: bool,
    /// A file naming the repository and its credentials, written onto
    /// the target so its first boot puts the home directory back.
    pub restore: Option<std::path::PathBuf>,
    pub yes: bool,
    pub json: bool,
}

/// What the person answered, and what the target will converge to.
pub struct Account {
    pub name: String,
    pub password_hash: String,
    pub groups: Vec<String>,
    /// None means whatever `useradd` defaults to, which is what a
    /// declaration that names no shell also gets.
    pub shell: Option<String>,
}

/// The file `kuma-user-sync` sources, in the format it already reads.
///
/// Deliberately the same shape as the baked `/usr/lib/kuma/user` rather
/// than a new one: the converger gains a second source, not a second
/// parser, and a machine installed this way is indistinguishable at boot
/// from one whose declaration named the account.
pub fn user_file(account: &Account) -> String {
    let mut out = format!("KUMA_USER='{}'\n", account.name);
    // Same key the baked declaration writes, so the converger cannot
    // tell the two apart. Absent rather than empty when unset: the sync
    // script tests `[ -n "${KUMA_SHELL:-}" ]`, so an empty value would
    // read as "set" and hand useradd nothing.
    if let Some(shell) = &account.shell {
        out.push_str(&format!("KUMA_SHELL='/usr/bin/{shell}'\n"));
    }
    if !account.groups.is_empty() {
        out.push_str(&format!("KUMA_GROUPS='{}'\n", account.groups.join(" ")));
    }
    out.push_str(&format!("KUMA_PASSWORD_HASH='{}'\n", account.password_hash));
    out
}

/// The one-layer image that carries the answers onto the target.
///
/// Into /var, not /etc. bootc fills /var from the image once at install
/// and never touches it again; /etc is three-way merged on every update,
/// and a file the installer shipped as image content is not a local
/// modification, so the merge against a published image that has no such
/// file deletes it. The account would outlive the file describing it.
///
/// 0600 on the user file for the same reason the baked one is: it holds a
/// password hash and only the root-run converger reads it. This image is
/// thrown away once the install finishes; nothing tags it for keeping.
pub fn install_containerfile(source: &str, account: &Account, restore: bool) -> String {
    let shell = account.shell.as_deref();
    // `useradd -s /usr/bin/nonsense` does not fail. It makes the account
    // with a shell that is not there and the machine comes up unable to
    // log anybody in, which is the exact failure this verb exists to
    // prevent. A declaration gets the same guard at image build time;
    // an install of somebody else's image has no build of its own until
    // this one, so this is where it goes. Only for an explicit --shell:
    // a shell the image declared was already checked when it was built.
    let guard = match shell {
        Some(shell) => format!("RUN test -x /usr/bin/{shell}\n"),
        None => String::new(),
    };
    format!(
        "# Generated by kuma for `kuma install`. One layer over the image\n\
         # being installed, carrying what the target converges to.\n\
         FROM {source}\n\
         {guard}\
         COPY --chmod=600 kuma-user /var/lib/kuma/user\n\
         COPY kuma-hostname /var/lib/kuma/hostname\n\
         {restore}{}",
        drop_foreign_autologin(&account.name),
        restore = if restore { RESTORE_LAYER } else { "" }
    )
}

/// The request and the credential, written onto the target exactly the
/// way the account is: bootc fills `/var` from the image once, at
/// install, and never again, which is what install-time answers need.
///
/// 0600 because it is a credential, and in `/var/lib/kuma/secrets` so it
/// sits where the machine's own credential would, under the fixed name
/// the first-boot unit looks for. The declaration's `secret` name is not
/// used here: the machine being restored has no declaration of its own
/// yet, and the file it is given is the one that names the repository.
const RESTORE_LAYER: &str =
    "COPY --chmod=600 kuma-restore-secret /var/lib/kuma/secrets/restore.env\n\
     COPY kuma-restore-request /var/lib/kuma/restore-request\n";

/// What a restore file has to say before an install will accept it.
///
/// `RESTIC_REPOSITORY` rather than a second flag, because the machine
/// being restored has no declaration yet and the address has to come
/// from somewhere. That makes the whole recovery one file: put it on the
/// stick beside the ISO and a dead disk needs nothing else typed.
///
/// Checked here rather than at first boot, where the only person who
/// could read the error has already walked away.
pub fn restore_file_is_usable(text: &str) -> Result<(), String> {
    let names: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| l.split_once('=').map(|(k, _)| k.trim()))
        .collect();
    if !names.contains(&"RESTIC_REPOSITORY") {
        return Err("the restore file must set RESTIC_REPOSITORY: the machine being restored has \
             no declaration yet, so the address has to come from the file"
            .to_string());
    }
    if !names.iter().any(|n| n.starts_with("RESTIC_PASSWORD")) {
        return Err("the restore file sets no RESTIC_PASSWORD (or RESTIC_PASSWORD_FILE), so \
             nothing could open the repository"
            .to_string());
    }
    Ok(())
}

/// Where the greeter autologins somebody who will not exist here.
///
/// An image built from a declaration with `autologin = true` bakes that
/// account's name into the greeter's config. Installed for a different
/// account, the name resolves to nobody: greetd fails
/// `pam_acct_mgmt: USER_UNKNOWN`, restarts five times, and gives up, so
/// the machine boots to a console nobody can log in at. Found on a real
/// install, where every other part had converged correctly.
///
/// Dropped rather than pointed at the new account, because kuma already
/// answered this question once: `kuma-user-sync` clears the account keys
/// between the baked declaration and the installer's file so that "an
/// image that declared a user cannot lend its password to somebody
/// else's account". Autologin is a property of that same account, and
/// lending it is the same move.
///
/// Cut at the section header, because kuma writes this block last in
/// both files it appears in, and because a greeter that has lost its
/// autologin still greets. Only when the names differ: installing an
/// image onto a machine for the account it already declares is the case
/// where autologin was meant.
///
/// The name goes into single quotes in generated shell. That is safe
/// because `ask_account` has already put it through `validate_name`,
/// which rejects quotes and every shell metacharacter; it is not safe on
/// its own, and this is the sentence that says so.
fn drop_foreign_autologin(name: &str) -> String {
    format!(
        "# A greeter cannot autologin an account this machine will not\n\
         # have. Runs inside the build, so nothing has to read the image\n\
         # from outside it.\n\
         RUN for conf in {} {}; do \\\n    \
         [ -f \"$conf\" ] || continue; \\\n    \
         grep -q '^\\[initial_session\\]' \"$conf\" || continue; \\\n    \
         [ \"$(sed -n 's/^user *= *\"\\(.*\\)\"/\\1/p' \"$conf\" | tail -1)\" = '{name}' ] \\\n      \
         || sed -i '/^\\[initial_session\\]/,$d' \"$conf\"; \\\n\
         done\n",
        crate::containerfile::GREETD_CONF,
        crate::containerfile::COSMIC_GREETER_CONF,
    )
}

/// What installing this image puts on the disk that a published one
/// would not.
///
/// A shared image declares no `[user]`, which is why the installer asks
/// for one. An image built from somebody's own declaration does declare
/// one, and installing it carries that account's name and password hash
/// onto the disk in the baked declaration, for an account this machine
/// will not even create. `kuma iso` already warns when a declared user
/// rides into installer media; this is the same hazard by the same
/// route, and it went unremarked until an install produced a machine
/// nobody could log in to.
pub fn baked_user_warning(baked: &str, installing: &str) -> Option<String> {
    let config: crate::config::Config = toml::from_str(baked).ok()?;
    let declared = config.user?;
    if declared.name == installing {
        return None;
    }
    let mut out = format!(
        "note: this image declares the account '{}', which is not the '{installing}' being\n\
         created here. Its password hash rides along in the image's baked declaration,\n\
         so this disk carries a credential for an account it will never have.",
        declared.name
    );
    if declared.autologin {
        out.push_str(
            "\nIts autologin has been dropped from the greeter, which would otherwise try to\n\
             log in a user that does not exist and fail to start at all.",
        );
    }
    out.push_str("\n\nImages meant to be installed elsewhere should declare no [user].");
    Some(out)
}

/// The hostname the installed machine takes.
///
/// Machine state, like the account: a published image cannot know it, and
/// every machine installed from one would otherwise answer to the same
/// name. Defaulted rather than demanded, because a name is the least
/// consequential thing being decided here.
pub fn ask_hostname(given: Option<String>) -> Result<String> {
    use std::io::IsTerminal;
    if let Some(name) = given {
        crate::config::validate_name(&name, "hostname", &['.', '-'])?;
        return Ok(name);
    }
    if !std::io::stdin().is_terminal() {
        return Ok(DEFAULT_HOSTNAME.to_string());
    }
    eprint!("Hostname for the installed machine [{DEFAULT_HOSTNAME}]: ");
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    let name = line.trim();
    if name.is_empty() {
        return Ok(DEFAULT_HOSTNAME.to_string());
    }
    crate::config::validate_name(name, "hostname", &['.', '-'])?;
    Ok(name.to_string())
}

/// Matches what every kuma image bakes, so accepting the default changes
/// nothing rather than writing a file that says what was already true.
pub const DEFAULT_HOSTNAME: &str = "kuma";

/// Reasons not to write to this disk, in the order a person would want
/// to hear them.
///
/// Pure over the mount table so the dangerous branch is testable without
/// a spare disk. Every other kuma verb is reversible: `switch` stages,
/// `rollback` exists, a bad build is a build. This one is not, so the
/// checks that stop it are the part worth being sure of.
pub fn disk_objections(
    disk: &str,
    proc_mounts: &str,
    lsblk_mountpoints: &str,
    to_file: bool,
) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    // A file target is a disk image, written through a loopback device.
    // It is not a device path and must not be judged as one, but the
    // mount checks below still run: a disk image that is currently
    // mounted somewhere is exactly as bad to overwrite as a disk.
    if !to_file && !disk.starts_with("/dev/") {
        out.push(format!("{disk} is not a device path"));
    }

    // lsblk first, because it is the only one that sees through LUKS and
    // LVM. /proc/mounts names the *mapper* device for an encrypted root,
    // so a disk whose every partition is inside a crypt container has no
    // line in it that mentions the disk at all. On a machine with /boot
    // encrypted too, a check built only on /proc/mounts finds nothing to
    // object to and cheerfully wipes the running system.
    for mount in lsblk_mountpoints.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if seen.insert(mount.to_string()) {
            out.push(format!("something on {disk} is in use at {mount}"));
        }
    }

    // And /proc/mounts, which needs no external command: if lsblk is
    // missing or fails, this is the whole guard rather than none of it.
    // Matches /dev/sda1 for /dev/sda and /dev/nvme0n1p1 for /dev/nvme0n1.
    for line in proc_mounts.lines() {
        let mut fields = line.split_whitespace();
        let (Some(source), Some(target)) = (fields.next(), fields.next()) else {
            continue;
        };
        let is_partition = source == disk
            || source.strip_prefix(disk).is_some_and(|rest| {
                let rest = rest.strip_prefix('p').unwrap_or(rest);
                !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit())
            });
        if is_partition && seen.insert(target.to_string()) {
            out.push(format!("{source} from {disk} is mounted at {target}"));
        }
    }
    out
}

/// Why this reference cannot be what a machine updates from.
///
/// `localhost/...` is not a registry anybody else can reach: on the
/// installed machine it means that machine, which has no registry
/// running, so the first `kuma update` fails with a connection refused
/// naming a host nobody meant. The image installs fine and the machine
/// is stranded on it forever, which is worth catching before a disk is
/// written rather than weeks later.
pub fn unreachable_update_source(reference: &str) -> Option<String> {
    if reference.starts_with("localhost/") {
        return Some(format!(
            "{reference} is local to the machine running this install.\n\n\
             The installed machine records it as where updates come from, and\n\
             `localhost` there means itself. It has no registry, so it would\n\
             never update.\n\n\
             Install a published image, or keep this one and say where the\n\
             machine should update from:\n\n  \
             --update-from ghcr.io/<owner>/kuma:niri"
        ));
    }
    None
}

/// Which image a bare `kuma install` writes, given what the media it is
/// running on recorded about itself, and what to say about the choice.
///
/// Installing pulls from a registry rather than copying the media, so
/// the media's own image is only installable when it came from one. The
/// three cases are worth keeping apart:
///
/// - Nothing recorded: an ordinary machine, or media built before kuma
///   recorded this. The published image, silently, as before.
/// - A registry reference: install what this media is. Somebody who
///   built media from their declaration and booted it was looking at
///   their own system, and installing something else is a surprise no
///   message makes acceptable.
/// - A `localhost/` reference: the common case, since `kuma build`
///   writes one. It cannot be pulled from anywhere, so the published
///   image is the only thing that can be installed, and the difference
///   has to be said out loud rather than discovered afterwards.
pub fn image_for_media(recorded: Option<&str>) -> (String, Option<String>) {
    let published = crate::PUBLISHED_IMAGE.to_string();
    let Some(recorded) = recorded.map(str::trim).filter(|r| !r.is_empty()) else {
        return (published, None);
    };
    if unreachable_update_source(recorded).is_none() {
        return (recorded.to_string(), None);
    }
    let note = format!(
        "This media was built from {recorded}, which is local to the machine\n\
         that built it and cannot be pulled from here. Installing {published}\n\
         instead, so the installed machine has somewhere to update from.\n\n\
         To install what this media is running, push that image to a registry\n\
         and name it: kuma install --image ghcr.io/<owner>/kuma:<tag>"
    );
    (published, Some(note))
}

/// A disk somebody might install onto, as `lsblk` describes it.
pub struct Disk {
    pub path: String,
    pub size: String,
    pub model: String,
    /// Everything mounted anywhere on it, found through partitions and
    /// through LUKS and LVM children. Non-empty means refuse.
    pub mounts: Vec<String>,
}

/// The disks worth offering, from `lsblk -J -o NAME,SIZE,MODEL,TYPE,MOUNTPOINTS`.
///
/// Pure over the JSON so the list a person chooses from is testable
/// without spare hardware, which matters more here than usual: choosing
/// wrong is not recoverable.
///
/// zram and loop devices are dropped. Both report `type: "disk"`, and
/// neither is a thing anyone can install onto: one is compressed RAM,
/// the other is a file. A list that offers them invites a mistake in the
/// one place a mistake is permanent.
pub fn disks_from_lsblk(json: &str) -> Result<Vec<Disk>> {
    fn mounts_of(node: &serde_json::Value, out: &mut Vec<String>) {
        if let Some(points) = node.get("mountpoints").and_then(|v| v.as_array()) {
            for point in points.iter().filter_map(|p| p.as_str()) {
                if !point.is_empty() {
                    out.push(point.to_string());
                }
            }
        }
        // Recursive, because the mount that matters is usually two levels
        // down: a partition holding a LUKS container holding the root.
        for child in node.get("children").and_then(|v| v.as_array()).into_iter().flatten() {
            mounts_of(child, out);
        }
    }

    let root: serde_json::Value =
        serde_json::from_str(json).context("cannot read the disk list from lsblk")?;
    let mut out = Vec::new();
    for dev in root.get("blockdevices").and_then(|v| v.as_array()).into_iter().flatten() {
        if dev.get("type").and_then(|v| v.as_str()) != Some("disk") {
            continue;
        }
        let name = dev.get("name").and_then(|v| v.as_str()).unwrap_or_default();
        if name.is_empty() || name.starts_with("zram") || name.starts_with("loop") {
            continue;
        }
        let mut mounts = Vec::new();
        mounts_of(dev, &mut mounts);
        out.push(Disk {
            path: format!("/dev/{name}"),
            size: dev.get("size").and_then(|v| v.as_str()).unwrap_or("?").to_string(),
            model: dev.get("model").and_then(|v| v.as_str()).unwrap_or("").trim().to_string(),
            mounts,
        });
    }
    Ok(out)
}

/// Ask which disk, listing what was found.
///
/// Never picks for you, not even when exactly one disk is free. Every
/// other kuma verb can be undone; this one writes a partition table.
/// A single-candidate machine is also the most likely place for the one
/// disk to be the one you are running from.
///
/// In-use disks stay on the list, marked and refused, rather than being
/// hidden. Hiding them makes somebody wonder where their disk went and
/// look for it among the ones that are left.
pub fn choose_disk(mut disks: Vec<Disk>) -> Result<Disk> {
    use std::io::{IsTerminal, Write};
    if !std::io::stdin().is_terminal() {
        bail!("no --disk given, and nothing to ask: pass --disk when not on a terminal");
    }
    if disks.is_empty() {
        bail!("no disks found to install onto");
    }
    println!("\nDisks on this machine:\n");
    for (index, disk) in disks.iter().enumerate() {
        let model = if disk.model.is_empty() { String::new() } else { format!("  {}", disk.model) };
        // Naming every mount is unreadable and adds nothing: an encrypted
        // root reports seven, and the reader needs to know the disk is
        // busy, not the shape of its filesystem tree.
        let state = if disk.mounts.is_empty() {
            String::new()
        } else {
            let shown = disk.mounts.iter().take(2).cloned().collect::<Vec<_>>().join(", ");
            let rest = disk.mounts.len().saturating_sub(2);
            let more = if rest > 0 { format!(" and {rest} more") } else { String::new() };
            format!("   in use at {shown}{more}  (refused)")
        };
        println!("  {}  {:<14} {:>8}{}{}", index + 1, disk.path, disk.size, model, state);
    }
    let free = disks.iter().filter(|d| d.mounts.is_empty()).count();
    if free == 0 {
        bail!("every disk here is in use; nothing can be installed onto safely");
    }
    loop {
        print!("\nWhich disk should kuma install onto? [1-{}, or q] ", disks.len());
        std::io::stdout().flush()?;
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line)? == 0 {
            bail!("no answer");
        }
        let answer = line.trim();
        if answer.eq_ignore_ascii_case("q") {
            bail!("nothing was changed");
        }
        match answer.parse::<usize>() {
            Ok(n) if n >= 1 && n <= disks.len() => {
                let disk = &disks[n - 1];
                if !disk.mounts.is_empty() {
                    println!("{} is in use at {}.", disk.path, disk.mounts.join(", "));
                    continue;
                }
                // The whole Disk, not its path: what it knows about being
                // mounted is the same question the objection check asks,
                // and asking twice is how two answers start to differ.
                return Ok(disks.swap_remove(n - 1));
            }
            _ => println!("Answer with a number from the list, or q to stop."),
        }
    }
}

/// EFI boot entries that appeared while installing, as `efibootmgr`
/// numbers them.
///
/// Installing writes a boot entry into the firmware of the machine doing
/// the installing, because `bootupctl` names the ESP it just wrote and
/// asks the firmware to remember it. That is exactly right for a disk
/// and useless for a file: the entry points at a partition inside an
/// image, sorts itself to the front of the boot order, and names a
/// device no firmware can ever find.
///
/// bootc has no flag for this. `--generic-image` skips the firmware but
/// also installs every bootloader type, and this layout has no BIOS Boot
/// Partition, so it fails at `grub2-install` after the disk is already
/// written. So kuma does not prevent the entry; it notices it and says
/// which one it is.
///
/// A diff of two `efibootmgr` listings rather than a search for kuma's
/// own name: the entry is named by the image's os-release, an install of
/// somebody else's image can call it anything, and the only thing kuma
/// knows for certain is that it was not there a minute ago.
pub fn new_efi_entries(before: &str, after: &str) -> Vec<String> {
    after
        .lines()
        .filter(|line| line.starts_with("Boot") && !before.lines().any(|old| old == *line))
        .filter_map(|line| {
            // `Boot0008* Kuma\tHD(1,GPT,...)`. The number is fixed width
            // and the rest is a device path nobody needs here.
            let rest = line.strip_prefix("Boot")?;
            let (number, _) = rest.split_at(rest.find(['*', ' '])?);
            (number.len() == 4 && number.chars().all(|c| c.is_ascii_hexdigit()))
                .then(|| number.to_string())
        })
        .collect()
}

/// Whether to encrypt the root, asked unless `--encrypt` already said so.
///
/// Not a default in either direction. Encryption on by default would
/// hand somebody a machine that stops at a passphrase prompt they never
/// asked for and cannot remove without reinstalling; off by default and
/// never asked is how a laptop ends up unencrypted because nothing
/// mentioned it. So the flag answers it early, a terminal is asked, and
/// a pipe with no flag gets the safe-to-be-wrong answer: an unencrypted
/// machine can be reinstalled, and a machine whose passphrase nobody
/// chose cannot be booted.
pub fn ask_encrypt(flagged: bool) -> Result<bool> {
    use std::io::{IsTerminal, Write};
    if flagged {
        return Ok(true);
    }
    if !std::io::stdin().is_terminal() {
        return Ok(false);
    }
    loop {
        // stderr, like `ask_account` and `ask_hostname`. A prompt on
        // stdout would land inside the JSON document that `--json --yes`
        // promises is the only thing there.
        eprint!("\nEncrypt this disk with a passphrase? [y/N] ");
        std::io::stderr().flush()?;
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line)? == 0 {
            return Ok(false);
        }
        match line.trim().to_ascii_lowercase().as_str() {
            "y" | "yes" => return Ok(true),
            "" | "n" | "no" => return Ok(false),
            _ => eprintln!("Answer y or n."),
        }
    }
}

/// The passphrase that unlocks the disk at every boot.
///
/// Asked twice on a terminal, because a mistyped one is not discovered
/// until the machine will not boot and there is nothing left to compare
/// it against. From stdin otherwise, one line, ahead of the account
/// password for the same reason it is asked first: it decides the shape
/// of the disk, and the account does not.
///
/// No minimum length and no strength opinion. Unlike everything else
/// decided here, this one *can* be changed later on the installed
/// machine (`cryptsetup luksChangeKey`), so a rule invented here would
/// be a rule nothing enforces afterwards.
pub fn ask_passphrase() -> Result<String> {
    use std::io::IsTerminal;
    let passphrase = if std::io::stdin().is_terminal() {
        let first = rpassword::prompt_password("Passphrase to unlock this disk at boot: ")?;
        let again = rpassword::prompt_password("Retype the disk passphrase: ")?;
        if first != again {
            bail!("passphrases don't match");
        }
        first
    } else {
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        line.trim_end_matches(['\r', '\n']).to_string()
    };
    if passphrase.is_empty() {
        bail!("empty passphrase: the disk would be encrypted with nothing");
    }
    Ok(passphrase)
}

/// Ask for the account the target will create at first boot.
///
/// Not a terminal: the name comes from `--user` and stdin supplies the
/// password, one line, nothing else. That is what keeps the password out
/// of argv where `ps` would show it, and it is why there is no
/// `--password` flag. Omitting `--user` there is an error rather than a
/// prompt nobody is present to answer.
pub fn ask_account(
    name: Option<String>,
    groups: Vec<String>,
    shell: Option<String>,
) -> Result<Account> {
    use std::io::IsTerminal;
    let interactive = std::io::stdin().is_terminal();
    let name = match name {
        Some(name) => name,
        None if interactive => {
            eprint!("Account name for the installed machine: ");
            let mut line = String::new();
            std::io::stdin().read_line(&mut line)?;
            line.trim().to_string()
        }
        None => bail!("no account name: pass --user, or run this on a terminal"),
    };
    crate::config::validate_name(&name, "user.name", &['.', '-', '_'])?;

    let password = if interactive {
        let first = rpassword::prompt_password(format!("Password for {name}: "))?;
        let again = rpassword::prompt_password("Retype to confirm: ")?;
        if first != again {
            bail!("passwords don't match");
        }
        first
    } else {
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        line.trim_end_matches(['\r', '\n']).to_string()
    };
    if password.is_empty() {
        bail!("empty password: the installed machine would have no way in");
    }
    if let Some(shell) = &shell {
        crate::config::validate_name(shell, "user.shell", &['.', '-', '_'])?;
    }
    Ok(Account { name, password_hash: crate::hash_password(&password)?, groups, shell })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account() -> Account {
        Account {
            name: "mira".into(),
            password_hash: "$6$abc$def".into(),
            groups: vec!["wheel".into()],
            shell: Some("fish".into()),
        }
    }

    /// The installer writes what the converger already reads. If these
    /// drift, an installed machine silently comes up with no account,
    /// which is the exact failure this verb exists to prevent.
    #[test]
    fn the_written_file_is_what_user_sync_sources() {
        let text = user_file(&account());
        assert!(text.contains("KUMA_USER='mira'"));
        assert!(text.contains("KUMA_GROUPS='wheel'"));
        assert!(text.contains("KUMA_PASSWORD_HASH='$6$abc$def'"));
        assert!(text.contains("KUMA_SHELL='/usr/bin/fish'"));
        // Unset means absent, not empty: the sync script treats an empty
        // KUMA_SHELL as set and would pass useradd nothing.
        let bare = Account { shell: None, ..account() };
        assert!(!user_file(&bare).contains("KUMA_SHELL"));
        // Shell-sourceable: one KEY='value' per line, nothing else.
        for line in text.lines() {
            assert!(line.contains("='"), "not a shell assignment: {line}");
        }
    }

    /// A mounted partition of the target is the one thing between this
    /// verb and destroying the medium it is running from. On live media
    /// the ISO is mounted, and a person who typed the wrong letter would
    /// otherwise find out afterwards.
    #[test]
    fn a_mounted_partition_of_the_target_is_an_objection() {
        let mounts = "\
/dev/nvme0n1p3 / btrfs rw 0 0
/dev/nvme0n1p2 /boot ext4 rw 0 0
/dev/sr0 /run/initramfs/live iso9660 ro 0 0
tmpfs /tmp tmpfs rw 0 0
";
        let objections = disk_objections("/dev/nvme0n1", mounts, "", false);
        assert_eq!(objections.len(), 2, "both partitions of the target");

        // A different disk is not an objection just because it exists.
        assert!(disk_objections("/dev/sda", mounts, "", false).is_empty());
        // ... and the partition-suffix match must not fire on a disk
        // whose name merely starts the same way.
        assert!(disk_objections("/dev/nvme0", mounts, "", false).is_empty());
    }

    /// The case /proc/mounts cannot see, and the one most likely to be
    /// somebody's actual laptop: every partition inside a LUKS container,
    /// so the mount table names /dev/mapper/luks-... and never the disk.
    /// Without lsblk this disk looks idle and gets wiped while running.
    #[test]
    fn an_encrypted_disk_with_no_direct_mounts_is_still_in_use() {
        let mounts = "\
/dev/mapper/luks-f5d1fc89 /sysroot btrfs rw 0 0
/dev/mapper/luks-f5d1fc89 /var btrfs rw 0 0
";
        assert!(
            disk_objections("/dev/nvme0n1", mounts, "", false).is_empty(),
            "the mount table genuinely cannot see this, which is the point"
        );
        let lsblk = "/var\n/sysroot\n\n/boot\n";
        let objections = disk_objections("/dev/nvme0n1", mounts, lsblk, false);
        assert_eq!(objections.len(), 3, "blank lines are unmounted partitions");
        assert!(objections.iter().all(|o| o.contains("in use at")));
    }

    /// Both sources naming the same mount point is one objection, not two.
    #[test]
    fn the_two_sources_do_not_double_report() {
        let mounts = "/dev/sda1 /boot ext4 rw 0 0\n";
        let objections = disk_objections("/dev/sda", mounts, "/boot\n", false);
        assert_eq!(objections.len(), 1);
    }

    /// zram reports `type: "disk"` and would otherwise appear in a list
    /// somebody picks from with no undo. It is compressed RAM.
    #[test]
    fn the_disk_list_offers_only_things_that_can_be_installed_onto() {
        let json = r#"{"blockdevices":[
          {"name":"zram0","size":"8G","model":null,"type":"disk","mountpoints":["[SWAP]"]},
          {"name":"loop0","size":"1G","model":null,"type":"disk","mountpoints":[]},
          {"name":"sr0","size":"1.5G","model":"QEMU DVD-ROM","type":"rom","mountpoints":["/run/initramfs/live"]},
          {"name":"sda","size":"932G","model":"ST1000LM035 ","type":"disk","mountpoints":[]}
        ]}"#;
        let disks = disks_from_lsblk(json).unwrap();
        assert_eq!(disks.len(), 1, "zram, loop and the optical drive are not targets");
        assert_eq!(disks[0].path, "/dev/sda");
        assert_eq!(disks[0].model, "ST1000LM035", "lsblk pads the model");
        assert!(disks[0].mounts.is_empty());
    }

    /// The mount that decides it is usually two levels down: a partition
    /// holding a LUKS container holding the root. A list built from the
    /// top level alone shows the running system's disk as free.
    #[test]
    fn a_disk_is_in_use_when_anything_nested_under_it_is_mounted() {
        let json = r#"{"blockdevices":[
          {"name":"nvme0n1","size":"476.9G","model":"Micron","type":"disk","mountpoints":[],
           "children":[
             {"name":"nvme0n1p1","size":"600M","type":"part","mountpoints":["/boot/efi"]},
             {"name":"nvme0n1p3","size":"474G","type":"part","mountpoints":[],
              "children":[{"name":"luks-abc","size":"474G","type":"crypt","mountpoints":["/sysroot","/var"]}]}
           ]}
        ]}"#;
        let disks = disks_from_lsblk(json).unwrap();
        assert_eq!(disks.len(), 1);
        assert_eq!(disks[0].mounts, vec!["/boot/efi", "/sysroot", "/var"]);
    }

    #[test]
    fn a_path_that_is_not_a_device_is_an_objection() {
        assert!(!disk_objections("nvme0n1", "", "", false).is_empty());
        assert!(!disk_objections("/home/me/disk.img", "", "", false).is_empty());
    }

    /// Installing to a file is installing to a disk image, which bootc
    /// writes through a loopback device. The device-path rule has to
    /// stand down for it, and every other check has to stay: a disk
    /// image someone has mounted is exactly as bad to overwrite as a
    /// disk, and more likely to be in use without being noticed.
    /// A machine that cannot update is not a machine anybody wants, and
    /// nothing about the install says so: it succeeds, boots, works, and
    /// fails the first time it is asked to take a new image.
    #[test]
    fn a_local_image_cannot_be_what_a_machine_updates_from() {
        assert!(unreachable_update_source("localhost/kuma:latest").is_some());
        assert!(unreachable_update_source("ghcr.io/someone/kuma:niri").is_none());
        // Not a prefix match on the word: a registry that merely starts
        // with those letters is somebody's real host.
        assert!(unreachable_update_source("localhost.example.com/kuma:v1").is_none());
    }

    /// What a bare `kuma install` writes, which was kuma's published
    /// image on every medium regardless of what that medium was. Someone
    /// could build installer media from their own declaration, boot it,
    /// look at their own desktop, install, and get a different system,
    /// with nothing anywhere saying so.
    #[test]
    fn a_bare_install_writes_the_media_it_can_actually_pull() {
        // An ordinary machine, and media built before this was recorded.
        let (image, note) = image_for_media(None);
        assert_eq!(image, crate::PUBLISHED_IMAGE);
        assert!(note.is_none(), "nothing recorded is not worth a paragraph");

        // Media built from something installable: install what you booted.
        let (image, note) = image_for_media(Some("ghcr.io/someone/kuma:cosmic"));
        assert_eq!(image, "ghcr.io/someone/kuma:cosmic");
        assert!(note.is_none());

        // The common case, since `kuma build` writes a localhost tag. The
        // published image is the only installable answer, and the
        // difference is said rather than left to be discovered.
        let (image, note) = image_for_media(Some("localhost/kuma:latest"));
        assert_eq!(image, crate::PUBLISHED_IMAGE);
        let note = note.expect("a substitution this surprising has to be announced");
        assert!(note.contains("localhost/kuma:latest"));
        assert!(note.contains(crate::PUBLISHED_IMAGE));
        assert!(note.contains("--image"), "and it has to say how to get what you built");

        // A file with a trailing newline is what `printf` writes, and an
        // empty one is a record that failed to say anything.
        assert_eq!(image_for_media(Some("ghcr.io/o/k:t\n")).0, "ghcr.io/o/k:t");
        assert_eq!(image_for_media(Some("  ")).0, crate::PUBLISHED_IMAGE);
    }

    #[test]
    fn a_file_target_is_allowed_but_not_excused() {
        assert!(disk_objections("/var/tmp/kuma.raw", "", "", true).is_empty());
        let mounted = "/dev/loop0 /mnt/img ext4 rw 0 0\n";
        assert!(
            !disk_objections("/var/tmp/kuma.raw", mounted, "/mnt/img\n", true).is_empty(),
            "a mounted image is still in use"
        );
    }

    /// The account rides in as a layer rather than being written after
    /// the fact, and the installed machine still tracks the published
    /// image. Losing either half is silent: the first gives a machine
    /// with no account, the second a machine that can never update.
    #[test]
    fn the_derived_layer_carries_the_answers_at_0600() {
        let bare = Account { shell: None, ..account() };
        let out = install_containerfile("ghcr.io/example/kuma:niri", &bare, false);
        assert!(out.contains("FROM ghcr.io/example/kuma:niri"));
        assert!(out.contains("COPY --chmod=600 kuma-user /var/lib/kuma/user"));
        assert!(out.contains("COPY kuma-hostname /var/lib/kuma/hostname"));
        // Nothing to check when nobody asked for a shell: whatever the
        // image declares was already checked when the image was built.
        assert!(!out.contains("test -x"));
        // /etc is three-way merged against the published image on every
        // update, and a file shipped as image content is not a local
        // modification, so the merge would delete both of these.
        assert!(!out.contains("/etc/kuma/"));
    }

    /// A greeter that autologins the image author's account cannot start
    /// on a machine that has a different one. Proven on a real install:
    /// `pam_acct_mgmt: USER_UNKNOWN` from `getpwnam(martin)`, five
    /// restarts, then no greeter at all.
    #[test]
    fn autologin_for_an_account_this_machine_lacks_is_removed() {
        let out = install_containerfile("localhost/kuma:latest", &account(), false);
        // Both files, because the two desktops autologin through
        // different ones and only one of them would be noticed.
        assert!(out.contains(crate::containerfile::GREETD_CONF));
        assert!(out.contains(crate::containerfile::COSMIC_GREETER_CONF));
        // Cut at the section header, which is where kuma writes it.
        assert!(out.contains(r"sed -i '/^\[initial_session\]/,$d'"));
        // And only when the name differs: an image installed for the
        // account it declares is the case autologin was written for.
        assert!(out.contains("= 'mira' ]"));
        // The greeter survives; only its autologin goes.
        assert!(!out.contains("rm -f /etc/greetd"));
    }

    /// The other half of the same hazard, which no code can undo: the
    /// A restore that cannot work is worth refusing while the old
    /// machine's disk is still the only copy of the data. At first boot
    /// the only person who could read the error has walked away.
    #[test]
    fn a_restore_file_that_could_not_open_the_repository_is_refused() {
        // The address has to be in the file: the machine being restored
        // has no declaration yet, so there is nowhere else for it.
        let no_repo = "RESTIC_PASSWORD=hunter2\n";
        assert!(restore_file_is_usable(no_repo).unwrap_err().contains("RESTIC_REPOSITORY"));

        let no_password = "RESTIC_REPOSITORY=b2:kuma\n";
        assert!(restore_file_is_usable(no_password).unwrap_err().contains("RESTIC_PASSWORD"));

        let usable = "# recovery\nRESTIC_REPOSITORY=s3:https://minio.example:9000/kuma\n\
                      RESTIC_PASSWORD=hunter2\nAWS_ACCESS_KEY_ID=k\nAWS_SECRET_ACCESS_KEY=s\n";
        restore_file_is_usable(usable).unwrap();

        // A password held in a file beside it counts, which is the
        // shape restic prefers and the one that keeps a secret out of
        // an environment children inherit.
        let by_file = "RESTIC_REPOSITORY=b2:kuma\nRESTIC_PASSWORD_FILE=/run/key\n";
        restore_file_is_usable(by_file).unwrap();
    }

    /// The request rides in the same layer as the account, because it is
    /// the same kind of thing: an install-time answer that /var carries
    /// once. Without --restore it must leave no trace, so an ordinary
    /// install cannot end up with a unit waiting for a file it will
    /// never get.
    #[test]
    fn a_restore_request_rides_the_install_layer_only_when_asked() {
        let plain = install_containerfile("ghcr.io/example/kuma:niri", &account(), false);
        assert!(!plain.contains("restore"), "{plain}");

        let restoring = install_containerfile("ghcr.io/example/kuma:niri", &account(), true);
        assert!(restoring
            .contains("COPY --chmod=600 kuma-restore-secret /var/lib/kuma/secrets/restore.env"));
        assert!(restoring.contains("/var/lib/kuma/restore-request"));
        // 0600 on the credential and not on the request: one is a
        // secret and the other is a flag saying to look for it.
        let secret_line = restoring
            .lines()
            .find(|l| l.contains("restore.env"))
            .expect("the credential is copied");
        assert!(secret_line.contains("--chmod=600"), "{secret_line}");
    }

    /// image carries a password hash for an account this disk will never
    /// create. `kuma iso` warns about the same thing for media.
    #[test]
    fn installing_someone_elses_image_says_what_rides_along() {
        let baked = "schema_version = 1\n[user]\nname = \"martin\"\n\
                     password_hash = '$6$abc$def'\nautologin = true\n";
        let warning = baked_user_warning(baked, "mira").expect("a declared user is worth saying");
        assert!(warning.contains("martin"));
        assert!(warning.contains("password hash"));
        assert!(warning.contains("autologin"));
        // Nothing to warn about when the image declares the same account
        // it is being installed for, or declares none at all.
        assert!(baked_user_warning(baked, "martin").is_none());
        assert!(baked_user_warning("schema_version = 1\n", "mira").is_none());
        // A declaration without autologin still carries the hash, and
        // still says so, but has no greeter line to explain.
        let quiet = "schema_version = 1\n[user]\nname = \"martin\"\n";
        let warning = baked_user_warning(quiet, "mira").unwrap();
        assert!(warning.contains("password hash") && !warning.contains("autologin"));
    }

    /// An explicit --shell is the one thing here the image has never
    /// seen, so it is the one thing this layer checks. It fails the
    /// build rather than the boot, which is the difference between an
    /// install that stops and a machine nobody can log into.
    #[test]
    fn an_asked_for_shell_is_checked_before_the_account_is_written() {
        let out = install_containerfile("ghcr.io/example/kuma:niri", &account(), false);
        let guard = out.find("RUN test -x /usr/bin/fish").unwrap();
        assert!(guard < out.find("COPY --chmod=600").unwrap());
    }

    /// The entry an install leaves in this machine's firmware, which is
    /// pollution when the target was a file. Found by what changed, not
    /// by name: the name comes from the image being installed.
    #[test]
    fn a_boot_entry_that_was_not_there_before_is_reported() {
        let before = "\
BootCurrent: 0003
BootOrder: 0003,0000
Boot0000* Windows Boot Manager\tHD(1,GPT,c0355a54)/\\EFI\\Microsoft\\Boot\\bootmgfw.efi
Boot0003* Fedora\tHD(1,GPT,f3035b31)/\\EFI\\fedora\\shimx64.efi
";
        let after = "\
BootCurrent: 0003
BootOrder: 0008,0003,0000
Boot0000* Windows Boot Manager\tHD(1,GPT,c0355a54)/\\EFI\\Microsoft\\Boot\\bootmgfw.efi
Boot0003* Fedora\tHD(1,GPT,f3035b31)/\\EFI\\fedora\\shimx64.efi
Boot0008* Kuma\tHD(1,GPT,b0180ce6)/\\EFI\\fedora\\shimx64.efi
";
        assert_eq!(new_efi_entries(before, after), vec!["0008"]);
        // A reordered boot order is not a new entry, and neither is an
        // unchanged listing.
        assert!(new_efi_entries(after, after).is_empty());
        assert!(new_efi_entries(before, before).is_empty());
        // Nothing to say when the firmware cannot be read at all.
        assert!(new_efi_entries("", "").is_empty());
    }

    /// The flag answers the question, and a pipe with no flag answers it
    /// the way that can be undone. Both directions matter: an install
    /// driven by a script must not stop on a prompt nobody is there for,
    /// and it must not quietly encrypt a disk with a passphrase nobody
    /// chose either.
    ///
    /// The terminal branch cannot be reached from a test, which is why
    /// the two branches that can are pinned here.
    #[test]
    fn encryption_is_answered_by_the_flag_or_left_off_when_nobody_is_asked() {
        assert!(ask_encrypt(true).unwrap());
        assert!(!ask_encrypt(false).unwrap(), "piped stdin, no flag");
    }

    /// A name is the least consequential thing being decided here, so it
    /// is defaulted rather than demanded, and the default matches what
    /// every kuma image already bakes.
    #[test]
    fn the_hostname_falls_back_rather_than_failing() {
        assert_eq!(ask_hostname(Some("workshop".into())).unwrap(), "workshop");
        // Piped stdin takes the default instead of blocking on a prompt
        // nobody is there to answer.
        assert_eq!(ask_hostname(None).unwrap(), DEFAULT_HOSTNAME);
        assert!(ask_hostname(Some("not a hostname".into())).is_err());
    }
}
