/**
 * Dry-run executor.
 *
 * Creates a worker, initializes it, runs a script against each of 1-3 CSV data
 * rows (each row is split via `row.split(",")` and passed as argv — the CSV
 * header is not included), collects results, and terminates the worker.
 * Stops on first error.
 */

import { createPyodideWorker } from "./pyodideWorker";

export interface DryRunResult {
  rows: { input: string; stdout: string; stderr: string; durationMs: number }[];
  totalDurationMs: number;
}

/**
 * Runs `script` against each of `csvRows` (1-3 rows). Each row is split via
 * `row.split(",")` and passed as `argv` (so the script sees
 * `sys.argv = ["<user-script>", ...fields]`); the CSV header is not passed
 * to the script. Returns a DryRunResult with per-row results and
 * totalDurationMs. Stops on first error. Terminates the worker when done.
 *
 * @param script - Python source code to execute
 * @param csvRows - 1 to 3 CSV data rows (caller slices); each row is split
 *                  on comma into argv fields (no RFC 4180 quoting support)
 */
export async function dryRun(
  script: string,
  csvRows: string[],
): Promise<DryRunResult> {
  if (csvRows.length < 1 || csvRows.length > 3) {
    throw new Error("dryRun requires 1 to 3 CSV rows");
  }

  const worker = createPyodideWorker();

  try {
    await worker.init();

    const rows: {
      input: string;
      stdout: string;
      stderr: string;
      durationMs: number;
    }[] = [];

    for (const row of csvRows) {
      // Stops on first error: exec rejects and the error propagates
      // out of this loop, through the try block, into the caller.
      const result = await worker.exec(script, row.split(","));

      rows.push({
        input: row,
        stdout: result.stdout,
        stderr: result.stderr,
        durationMs: result.durationMs,
      });
    }

    const totalDurationMs = rows.reduce((acc, r) => acc + r.durationMs, 0);
    return { rows, totalDurationMs };
  } finally {
    worker.terminate();
  }
}
