# For agents

The self-describing principle is an API. An agent with a shell can operate
a kuma machine without kuma-specific knowledge, because every response
names the legal next commands.

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
- **Write.** `kuma schema` prints the JSON Schema for `kuma.toml`, generated
  from the same types that parse it, so it cannot drift from reality.
- **Mutate.** `build`, `switch`, `update`, `rollback`, `sync`, `add`,
  `capture`, `remove`, `clean`, and `install` accept `--json` and emit
  exactly one document on stdout: `{"ok": true, …}` with result fields and
  next `actions`, or `{"ok": false, "error": …}` with a non-zero exit.
  Progress and subprocess output move to stderr.
- **Nothing changes what's running without a reboot,** with one exception
  worth knowing before you drive it. `switch`, `update` and `rollback` gate
  on `--yes` and even then only stage a deployment. `install` writes a disk
  immediately and cannot be undone: no staged deployment to discard, no
  rollback slot. It still dry-runs by default, and its dry run reports the
  disk, the image, whether that image is already local, the partition
  `layout` it will write, whether the root will be `encrypted`, and what
  it will ask a person for (`asks`) before `--yes` does anything. It
  refuses a disk
  with anything mounted on it, and it is the only verb here that prompts:
  with `--user`, `--hostname` and `--disk` given, the password is the one
  remaining answer and is read from stdin rather than a flag, so it never
  reaches `ps` or a shell history. `--encrypt` adds a second, and stdin is
  then the disk passphrase first and the account password second, in the
  order the two are asked. Without the flag, an install driven this way is
  never encrypted: the question is only put to a terminal.

Without `--config`, kuma reads `./kuma.toml`, falling back to
`~/.config/kuma/kuma.toml`. Neither is ever created implicitly. With no
working copy at all, read-only commands fall back to the machine's baked
declaration, so an ISO-installed machine can `kuma update --yes` without
ever creating a file; editing is what requires one.

