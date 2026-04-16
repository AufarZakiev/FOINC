/**
 * Dry-run executor.
 *
 * Creates a worker, initializes it, runs a script against each of 1-3 CSV rows
 * (each prefixed with the header), collects results, and terminates the worker.
 * Stops on first error.
 */

import { createPyodideWorker } from "./pyodideWorker";

export interface DryRunResult {
  rows: { input: string; stdout: string; stderr: string; durationMs: number }[];
  totalDurationMs: number;
}

/**
 * Runs `script` against each of `csvRows` (1-3 rows), each prefixed with `header`.
 * Returns a DryRunResult with per-row results and totalDurationMs.
 * Stops on first error. Terminates the worker when done.
 *
 * @param script - Python source code to execute
 * @param csvRows - 1 to 3 CSV data rows (caller slices)
 * @param header - CSV header line to prepend to each row
 */
export async function dryRun(
  script: string,
  csvRows: string[],
  header: string,
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
      const stdinData = header + "\n" + row;
      // Stops on first error: exec rejects and the error propagates
      // out of this loop, through the try block, into the caller.
      const result = await worker.exec(script, stdinData);

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
