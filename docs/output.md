# Output

Every byte `omw` emits belongs to exactly one of three channels.

| Channel       | What it is                                                                    | Where it goes                  |
| ------------- | ----------------------------------------------------------------------------- | ------------------------------ |
| **Data**      | The command's product: schema JSON, scaffold, built wasm paths, test verdicts | stdout                         |
| **Logs**      | Diagnostics from `tracing`: lifecycle, flow, wire, errors                     | stderr, journald, or TUI panes |
| **Live view** | Ephemeral human feedback: progress, tabs, status                              | the terminal only (TUI)        |

The channels never share a stream at the same time:

- On a terminal the **live view** owns the screen; logs are rendered _inside_
  it, not written raw.
- When not a terminal there is no live view; logs go to stderr (or journald) and
  data goes to stdout.
- Under journald there is no live view; logs go to the journal and data to
  stdout.

This is why test verdicts live on stdout and logs on stderr: it is the only
split that keeps `omw-test run | ...` parseable while logs stream separately,
and it removes the clash between human status and JSONL diagnostics.

## Log formats

One global flag selects the format:

```text
--log-format auto|tty|pipe|journald
```

`auto` is the default and guesses; every other value is explicit.

| Format     | Output                      | Use              |
| ---------- | --------------------------- | ---------------- |
| `auto`     | picks one of the below      | default          |
| `tty`      | ratatui TUI on the terminal | interactive use  |
| `pipe`     | JSONL to stderr             | CI, Docker, Loki |
| `journald` | native journald fields      | systemd services |

`auto` detection, in order:

1. stdout and stderr are both terminals → `tty`.
2. `JOURNAL_STREAM` is set (systemd has connected stdio to the journal) →
   `journald`.
3. otherwise → `pipe`.

The terminal check comes first: a terminal server (or any systemd-managed
session) can leak `JOURNAL_STREAM` into a process whose stderr is a real TTY,
which would otherwise hide the TUI behind a journald choice.

Both streams must be terminals for `tty` so that redirecting stdout (say
`omw-test run > results.txt`) never springs a full-screen TUI on you.
`--log-format tty` forces the TUI when stderr is a terminal even if stdout is
redirected (the view renders on stderr, data still goes to stdout); when stderr
is not a terminal — or the terminal cannot otherwise be driven — the live view
falls back to `pipe`.

There is deliberately no `OMW_LOG_FORMAT`, no `--no-tui`/`OMW_TUI`, and no
`--color`:

- To pin a format, pass `--log-format $OMW_LOG_FORMAT` (or the literal value).
- To avoid the TUI, choose another format; the TUI _is_ the `tty` format.
- Color is not a knob: `tty` is colored (unless `NO_COLOR` is set) and
  `pipe`/`journald` are structured data where color is meaningless.

### Levels

The level is controlled by `RUST_LOG` (through `tracing-subscriber`'s
`EnvFilter`), with a per-command default. There are no `-q`/`-v` flags; the
common cases are easy to express with `RUST_LOG`:

| Want                | Set                           |
| ------------------- | ----------------------------- |
| quiet (like `-q`)   | `RUST_LOG=warn`               |
| default             | unset                         |
| verbose (like `-v`) | `RUST_LOG=debug`              |
| trace (like `-vv`)  | `RUST_LOG=trace`              |
| one target          | `RUST_LOG=omw=debug,mcp=info` |

`omw-test` is quiet on success by construction rather than by level: each test
runs under a `test = <label>` span, its logs are buffered, and a failure prints
the assertion that broke and replays those logs. This works in every format,
including `pipe`.

### `NO_COLOR`

If the `NO_COLOR` environment variable is set (to anything), the TUI renders
monochrome. This is the cross-tool convention from no-color.org; the TUI is the
only colored surface omw has.

## The matrix

| Consumer →       | `tty` (interactive)                 | `pipe` (CI/Docker/Loki)              | `journald`                           |
| ---------------- | ----------------------------------- | ------------------------------------ | ------------------------------------ |
| logs             | captured into TUI panes             | JSONL → stderr                       | native fields                        |
| live view        | info + source/log panes + gauge     | none                                 | none                                 |
| verdicts/data    | verdicts + failing assertion in TUI | stdout (verdict + failing assertion) | stdout (verdict + failing assertion) |
| MCP child stderr | `mcp:<name>` tab                    | `source`/`tooling` JSONL             | `source`/`tooling` fields            |
| MCP child stdout | never captured (JSON-RPC wire)      | never captured                       | never captured                       |
| errors           | rendered once in the TUI            | one JSONL line → stderr              | error field                          |

MCP child processes do not write to omw's file descriptors: their stderr is
re-emitted as `tracing` events tagged with `source = "mcp"` and
`tooling = <name>`, so it flows through the same policy as everything else
(native fields under journald, JSON fields for observability, an `mcp:<name>`
tab in the TUI). Their stdout is the JSON-RPC wire and is never captured.

Errors are reported exactly once through the active format, so there is no
duplicate `tracing::error!` plus `Error: ...` report.

A supervisor-driven abort — a `shutdown`, `reload` or `stop` interrupting a
brain mid-run — is not a failure. Those are logged at `debug` (or rendered as
the brain's own lifecycle event), so Ctrl-C does not paint a scary `ERROR` line
while a shutdown is in progress.

## Per-command behavior

| Command                 | Data (stdout)                                                   | Logs                                   | Live view      |
| ----------------------- | --------------------------------------------------------------- | -------------------------------------- | -------------- |
| `omw run`               | —                                                               | format-selected                        | TUI on `tty`   |
| `omw loop`              | —                                                               | format-selected                        | TUI on `tty`   |
| `omw schema`            | JSON schema                                                     | `pipe`                                 | —              |
| `omw scaffold`          | TOML config / output path                                       | `pipe`                                 | —              |
| `omw-test run`          | `PASS`/`FAIL` + tally + first failure's trace/cursor/mock state | buffered per test, replayed on failure | gauge on `tty` |
| `omw-test schema`       | JSON schema                                                     | `pipe`                                 | —              |
| `omw-test compile-wasm` | built wasm paths                                                | `pipe`, `source = "cargo"`             | —              |

The TUI only engages for the long-running commands (`run`, `loop`,
`omw-test run`). `schema` and `scaffold` are data producers and always log on
`pipe`; a resolved `tty` never turns into a full-screen view for them.

## The live view

On a terminal, a custom `tracing` layer formats each event into a per-`source`
ring buffer and a render thread drives the alternate screen, so buffered logs
never corrupt the frame. Every pane is drawn with a single rounded border.

Both views open with an info panel: the launched command line (`$ <argv>` above
the details) and a details line joining the run's key facts with `·`. Bottom
lines pair a spinner and status on the left with key hints on the right.

- **`omw-cli`** (`run`, `loop`): a left "Sources" list of the `omw` tab plus one
  per MCP tooling server, and a right log pane titled by the active source. The
  details line shows the config path, agent count and endpoint `listen`.
  `←`/`→`/`Tab` switch source, `↑`/`↓` scroll.
- **`omw-test`** (`run`): a left "Tests" list marking every discovered test
  `✅`/`❌` (with a spinner on the running one), a right "Logs" pane for the
  running test or the failed test's assertion diff and buffered logs, and a
  bordered `N/M tests` gauge. The details line shows the discovery path, test
  count and (on an incremental `--watch` pass) the affected count. `--watch`
  retains every discovered test with its last verdict and re-runs only the
  affected tests each pass.

The TUI uses `✅`/`❌`; the product on stdout/pipe stays `PASS`/`FAIL` for
scripts. Keybindings are `←`/`→`/`Tab`/`h`/`l` switch, `↑`/`↓`/`j`/`k` scroll,
`q`/`Esc`/Ctrl-C quit.

The redraw interval and the per-tab log cap are the `tui_tick_ms` and
`tui_tab_capacity` [tunables](./tunables.md) (`0` capacity means unlimited), so
a busy terminal can redraw slower and a wide one can keep more scrollback.

When the live view tears down, `omw-test` replays the verdict lines and the
final tally to stdout, so an interactive run leaves its result in scrollback
instead of only a flash on the alternate screen. The failure diagnostics — the
trace, the assertion cursor and the mock queue state — are covered in
[Testing](./testing/testing.md#failure-diagnostics).
