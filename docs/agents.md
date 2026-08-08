# For agents

The self-describing principle is an API. An agent with a shell can operate
a kuma machine without kuma-specific knowledge, because every response
names the legal next commands.

- **Probe.** `kuma --json` is the root resource: state, facts, and `actions`
  as `{rel, cmd, why}`. Execute an action's `cmd` verbatim, then re-probe.
  `doctor --json` and `diff --json` carry findings with their fixes in the
  same shape.
- **Ask before doing.** `check --json` validates a declaration,
  `update --check --json` reports whether the base moved, `diff --json`
  reports drift. All three change nothing.
- **Write.** `kuma schema` prints the JSON Schema for `kuma.toml`, generated
  from the same types that parse it, so it cannot drift from reality.
- **Mutate.** `build`, `switch`, `update`, `rollback`, `sync`, `add`,
  `capture`, `remove`, and `clean` accept `--json` and emit exactly one
  document on stdout: `{"ok": true, …}` with result fields and next
  `actions`, or `{"ok": false, "error": …}` with a non-zero exit. Progress
  and subprocess output move to stderr.
- **Nothing changes what's running without a reboot.** The verbs that touch
  the system (`switch`, `update`, `rollback`) gate on `--yes` and even then
  only stage a deployment.

Without `--config`, kuma reads `./kuma.toml`, falling back to
`~/.config/kuma/kuma.toml`. Neither is ever created implicitly. With no
working copy at all, read-only commands fall back to the machine's baked
declaration, so an ISO-installed machine can `kuma update --yes` without
ever creating a file; editing is what requires one.

