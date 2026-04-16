/**
 * Dry-run executor.
 *
 * Creates a worker, initializes it, runs a script against each of 3 CSV rows
 * (each prefixed with the header), collects results, and terminates the worker.
 * Stops on first error.
 */

import { createPyodideWorker } from "./pyodideWorker";

export interface DryRunResult {
  rows: { input: string; output: string; durationMs: number }[];
  totalDurationMs: number;
}

/**
 * Runs `script` against each of the first 3 `csvRows`, each prefixed with `header`.
 * Returns a DryRunResult with per-row results and totalDurationMs.
 * Stops on first error. Terminates the worker when done.
 *
 * @param script - Python source code to execute
 * @param csvRows - Exactly 3 CSV data rows (caller slices)
 * @param header - CSV header line to prepend to each row
 */
export async function dryRun(
  script: string,
  csvRows: string[],
  header: string,
): Promise<DryRunResult> {
  if (csvRows.length !== 3) {
    throw new Error(`dryRun requires exactly 3 CSV rows, got ${csvRows.length}`);
  }

  const totalStart = performance.now();
  const worker = createPyodideWorker();

  try {
    await worker.init();

    const rows: { input: string; output: string; durationMs: number }[] = [];

    for (const row of csvRows) {
      const stdinData = header + "\n" + row;
      // Stops on first error: exec rejects and the error propagates
      // out of this loop, through the try block, into the caller.
      const result = await worker.exec(script, stdinData);

      rows.push({
        input: stdinData,
        output: result.stdout,
        durationMs: result.durationMs,
      });
    }

    const totalDurationMs = performance.now() - totalStart;
    return { rows, totalDurationMs };
  } finally {
    worker.terminate();
  }
}
