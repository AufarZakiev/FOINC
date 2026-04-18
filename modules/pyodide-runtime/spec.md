# Module: Pyodide Runtime

## Purpose
Browser-side Web Worker that loads Pyodide, executes a scientist's Python script against CSV rows using an argv/stdout contract, and performs timed dry runs on 1 to 3 rows.

## State Machine

### Worker Lifecycle

| State | Event | → State | Side effect |
|-------|-------|---------|-------------|
| Unloaded | `init` message received | Loading | Begin fetching Pyodide from `cdn.jsdelivr.net/pyodide/` |
| Loading | Pyodide ready | Idle | Post `{ type: "ready" }` to host |
| Loading | Pyodide fetch/init fails | Error | Post `{ type: "error", message }` to host |
| Idle | `exec` message received | Running | Patch `sys.argv = ['<user-script>', ...argv]`; patch `sys.stdin` to empty `StringIO('')`; redirect `sys.stdout`/`sys.stderr`; start `performance.now()` timer; start 30 s timeout timer |
| Running | Script completes | Idle | Post `{ type: "result", stdout, stderr, durationMs }` to host |
| Running | Script raises exception | Idle | Post `{ type: "error", message }` to host (message = traceback string) |
| Running | stdout exceeds 10 MB | Idle | Terminate execution; post `{ type: "error", message: "stdout limit exceeded (10 MB)" }` |
| Running | 30 s timeout fires | Terminated | Terminate the Worker entirely; host receives no further messages from this worker instance |
| Error | — | — | Terminal; host must create a new Worker to retry |
| Terminated | — | — | Terminal; host must create a new Worker to retry |

### Dry Run

| State | Event | → State | Side effect |
|-------|-------|---------|-------------|
| Validating | `dryRun(script, csvRows)` called with 1-3 rows | Initializing | Create Worker; call `init()` |
| Validating | `dryRun` called with 0 or >3 rows | Failed | Return error "dryRun requires 1 to 3 CSV rows" to caller; do not create Worker |
| Initializing | Worker posts `ready` | Executing | Send first `exec` message |
| Initializing | Worker init fails (enters Error state) | Failed | Return init error to caller; terminate Worker |
| Executing | All rows return results | Complete | Return `DryRunResult` to caller; terminate Worker |
| Executing | Any row returns error | Failed | Return the error to caller; do not execute remaining rows; terminate Worker |
| Executing | Worker enters Terminated state | Failed | Return timeout error to caller |

## API / Interface

### Worker postMessage Protocol

**Host → Worker**

`init`
```json
{ "type": "init" }
```
Triggers Pyodide loading. Must be sent exactly once after Worker creation.

`exec`
```json
{
  "type": "exec",
  "script": "<python source code>",
  "argv": ["1", "3"]
}
```
Executes the script with `sys.argv = ["<user-script>", ...argv]` and `sys.stdin` patched to empty `StringIO("")`. `argv` is the CSV data row pre-split by the host into fields (header is NOT included). Only valid when Worker is in `Idle` state.

Each `exec` call carries the script plus the CSV row pre-split into fields. For a dry run of N rows (1-3), `dryRun` issues N `exec` calls, each with one pre-split `argv` array.

**Worker → Host**

`ready`
```json
{ "type": "ready" }
```
Pyodide loaded and initialized.

`result`
```json
{
  "type": "result",
  "stdout": "<captured stdout>",
  "stderr": "<captured stderr>",
  "durationMs": 42.7
}
```
`durationMs`: wall-clock time of script execution measured via `performance.now()`.

`error`
```json
{
  "type": "error",
  "message": "<human-readable error string>"
}
```
Sent on Pyodide load failure, script exception, or stdout limit exceeded.

### Exported TypeScript Functions

`DryRunResult` and `ExecResult` are module-internal TypeScript types (not in `integrations/`); their shapes are defined inline below as part of the function signatures that expose them.

`createPyodideWorker(): PyodideWorker`
Creates and returns a Worker wrapper. Does not send `init` — caller must call `init()` on the returned handle.

`PyodideWorker.init(): Promise<void>`
Sends the `init` message; resolves when `ready` is received; rejects on `error`.

`PyodideWorker.exec(script: string, argv: string[]): Promise<ExecResult>`
Sends an `exec` message; resolves with `{ stdout: string, stderr: string, durationMs: number }`; rejects on `error` or timeout. If the 30 s timeout fires, the Worker is terminated and subsequent calls reject immediately.

`PyodideWorker.terminate(): void`
Forcibly terminates the underlying Web Worker.

`dryRun(script: string, csvRows: string[]): Promise<DryRunResult>`
Creates a Worker, initializes it, splits each row in `csvRows` via `row.split(",")` to produce an `argv` array, executes `script` against each `argv` (no header), and collects results. Returns `DryRunResult`. Terminates the Worker when done. `csvRows` must contain 1, 2, or 3 rows; otherwise rejects with "dryRun requires 1 to 3 CSV rows".

CSV-split limitation: values containing commas are not supported; rows are split on the first and every subsequent comma without quoting/escaping awareness (not RFC 4180-compliant).

`DryRunResult` shape:
```
{
  rows: Array<{ input: string; stdout: string; stderr: string; durationMs: number }>;
  totalDurationMs: number;
}
```
`rows[i].input` is the raw CSV data line for row `i`. `totalDurationMs` is the sum of `rows[*].durationMs`.

### UI Components

Vue 3 + TypeScript Single File Components under `modules/pyodide-runtime/ui/`.

This module does not accept files or user text input — that is the upload module's job. UI components here consume an `UploadCompleted` payload (see `integrations/ui/events.ts`) and run a dry run against it.

`DryRunResults.vue`
Displays the outcome of a dry run.
- Props: `result: DryRunResult`.
- Emits: none.
- UI: a table with one row per CSV data row, columns: input (the raw CSV line), stdout (a `<pre>` block), stderr (a `<pre>` block), `durationMs`. Footer shows `totalDurationMs`.

`DryRunPanel.vue`
Runs a dry run for an already-uploaded job and renders the outcome.
- Props: `upload: UploadCompleted` (from `integrations/ui/events.ts`). Required.
- Emits: none.
- State: `loading: boolean`, `error: string | null`, `result: DryRunResult | null`.
- Data flow:
  1. From `upload.csv`, split on `\n`, trim each line, discard lines empty after trimming. Drop the first remaining line (CSV header). The next 1-3 lines become `csvRows`.
  2. If parsing yields 0 rows, render an inline error `"CSV must contain at least 1 data row"` and do not call `dryRun`.
  3. If parsing yields >3 rows, use only the first 3 (caller slice matches `dryRun`'s contract).
  4. Render a "Run dry run" button. On click: set `loading=true`, call `dryRun(upload.script, csvRows)`; on resolve store `result` and render `DryRunResults`; on reject store `error.message` and render inline.
- Only one of idle-button / loading / error / result is visible at a time.
- When the `upload` prop changes to a new payload, reset `error` and `result` to `null`.

### Resource Limits

| Resource | Limit | Enforcement |
|----------|-------|-------------|
| Execution time per `exec` call | 30 s | Worker termination via `Worker.terminate()` |
| stdout size per `exec` call | 10 MB | Checked during stdout capture; execution aborted if exceeded |
| WASM memory | 256 MB | Managed by Pyodide internally (WASM linear memory limit) |

### Sandbox Restrictions

The Python environment has no access to:
- Filesystem (no `open()`, no `pathlib`)
- Network (no `urllib`, no `requests`, no sockets)
- `subprocess`, `os.system`, `ctypes`

These are enforced by not providing the relevant Emscripten/Pyodide modules. Pyodide packages are loaded lazily on first `import` within a script (not eagerly at init).

## Non-goals
- No file upload, file picker, or text input in UI — the module consumes an already-uploaded `UploadCompleted`; sourcing files is the upload module's responsibility.
- No aggregation detection, persistent state between `exec` calls, or backend component — each execution is isolated and runs entirely in the browser.
- No more than 3 rows per dry run, no partial results on failure, and no CSV-structured rendering of script output — stdout is shown as raw text in a `<pre>` block.
- No mounting of `DryRunPanel` — composition and routing live in `frontend/`, not in this module.
- No RFC 4180-compliant CSV parsing — commas are a hard field separator; quoted fields, escaped quotes, and embedded newlines are not recognized.
