# For agents

This is the interface for driving kuma from a program rather than by hand.
Every command speaks `--json`, and every response names the legal next
commands, so a caller can follow the output instead of encoding kuma's rules.

The loop is: probe, execute one of the actions it named, probe again.

- **Probe.** `kuma --json` is the root resource: state, facts, and `actions`
  as `{rel, cmd, why}`. Execute an action's `cmd` verbatim, then re-probe.
  `doctor --json` and `diff --json` carry findings with their fixes in the
  same shape. The `facts` keys are always `config`, `image` and `machine`,
  whatever the state says.
- **A live session says so.** Booted from installer media, the state is
  `live` and the facts describe media rather than a machine. Nothing is
  converging there and nothing persists, so most of what `doctor` grades
  does not apply and it reports one line instead of failing checks that
  were never going to pass. Its one action is `kuma install`, bare: the
  disk is not knowable from there, so the affordance is the form that
  lists what it found and asks.
- **A converging machine says so.** A first boot spends minutes installing
  declared apps, and for that whole window the machine genuinely does not
  match its declaration. The state is `converging`, not `drifted`, and no
  `sync` action is offered because a sync is what is running. Re-probe
  rather than acting: the machine is already doing the thing an agent
  would otherwise tell it to do.
- **Ask before doing.** `check --json` validates a declaration,
  `update --check --json` reports whether the base moved (or, for a composed
  base, every package that has and its advisory severity), `diff --json` reports
  drift, and `snapshot --json` lists what this machine has kept. All four
  change nothing, and `snapshot --restore --json` stays a dry run naming
  what it would overwrite until `--yes`.
- **A Fedora release change is a separate field, not a big diff.** `update`
  and `update --check` carry `fedora_release` with `current`, `changed`,
  `from` and `to`. Read that rather than inferring a distro upgrade from the
  size of the package list, and treat `changed: true` as needing a human
  even where you would otherwise stage automatically. Only `update` can
  report a move. `update --check` neither composes nor pulls, so it has
  nothing to compare against and always answers `changed: false` with
  `from` and `to` null: read `current` from a check and `changed` from an
  update. `current` is null when the release could not be read, which is
  not the same as a release that did not change, and is also what a
  machine with no base image in local storage reports, since a check does
  not download one to answer.
- **Reporting a broken machine.** `doctor --report` is `doctor --json` plus
  `kuma.version`, a `machine` object (`pretty_name`, `version_id`,
  `booted_image`, `booted_digest`, `staged`, `rollback`, `live_media`), and
  the `declaration` the machine was built from. `checks` and `summary` keep
  the same shape and place, so anything already reading `--json` reads a
  report unchanged. `user.password_hash` is removed before it prints, and a
  declaration kuma cannot parse arrives as `declaration.omitted` rather than
  as raw text. That is the payload to attach to a bug report.
- **Write.** `kuma schema` prints the JSON Schema for `kuma.toml`, generated
  from the same types that parse it, so it cannot drift from reality.
- **Mutate.** `build`, `switch`, `update`, `rollback`, `sync`, `add`,
  `capture`, `remove`, `clean`, and `install` accept `--json` and emit
  exactly one document on stdout: `{"ok": true, …}` with result fields and
  next `actions`, or `{"ok": false, "error": …}` with a non-zero exit.
  Progress and subprocess output move to stderr.
- **Nothing changes what's running without a reboot.** `switch`, `update` and
  `rollback` gate on `--yes`, and even then only stage a deployment.

`kuma install` is the one exception, and worth understanding before driving
it. It writes a disk immediately and cannot be undone: no staged deployment
to discard, no rollback slot. Three things make it drivable anyway:

- **It dry-runs by default.** Without `--yes` it reports the disk, the image,
  whether that image is already local, the partition `layout` it will write,
  whether the root will be `encrypted`, and what it will ask a person for
  (`asks`). It also refuses a disk with anything mounted on it.
- **It is the only verb that prompts.** Give it `--user`, `--hostname` and
  `--disk` and the password is the one remaining answer, read from stdin
  rather than a flag, so it never reaches `ps` or a shell history.
- **Encryption is opt-in from a program.** `--encrypt` makes stdin two lines,
  the disk passphrase first and the account password second, in the order a
  person is asked. Without the flag an install driven this way is never
  encrypted, because the question is only put to a terminal.

Without `--config`, kuma reads `./kuma.toml`, falling back to
`~/.config/kuma/kuma.toml`. Neither is ever created implicitly. With no
working copy at all, read-only commands fall back to the machine's baked
declaration, so an ISO-installed machine can `kuma update --yes` without
ever creating a file; editing is what requires one.

