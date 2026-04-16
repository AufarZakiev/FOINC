import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import type { ExecResult } from "../pyodideWorker";

// ---------------------------------------------------------------------------
// Mock createPyodideWorker before importing dryRun
// ---------------------------------------------------------------------------

const mockInit = vi.fn<() => Promise<void>>();
const mockExec = vi.fn<(script: string, stdinData: string) => Promise<ExecResult>>();
const mockTerminate = vi.fn();

vi.mock("../pyodideWorker", () => ({
  createPyodideWorker: () => ({
    init: mockInit,
    exec: mockExec,
    terminate: mockTerminate,
  }),
}));

// Import after mock is set up
import { dryRun } from "../dryRun";

// ---------------------------------------------------------------------------
// dryRun tests
// ---------------------------------------------------------------------------

describe("dryRun()", () => {
  const script = "import sys; print(sys.stdin.readline(), end='')";
  const header = "name,age";
  const rows = ["Alice,30", "Bob,25", "Carol,40"];

  beforeEach(() => {
    mockInit.mockReset();
    mockExec.mockReset();
    mockTerminate.mockReset();

    // Default: init succeeds
    mockInit.mockResolvedValue(undefined);

    // Default: exec returns a valid result
    mockExec.mockImplementation(async (_script: string, stdinData: string) => ({
      stdout: stdinData,
      stderr: "",
      durationMs: 10,
    }));
  });

  // We need to mock performance.now for totalDurationMs assertions
  // but the exact value doesn't matter for most tests.

  it("throws if csvRows.length is not 3 (too few)", async () => {
    await expect(dryRun(script, ["row1", "row2"], header)).rejects.toThrow(
      "dryRun requires exactly 3 CSV rows, got 2",
    );
  });

  it("throws if csvRows.length is not 3 (too many)", async () => {
    await expect(
      dryRun(script, ["r1", "r2", "r3", "r4"], header),
    ).rejects.toThrow("dryRun requires exactly 3 CSV rows, got 4");
  });

  it("throws if csvRows is empty", async () => {
    await expect(dryRun(script, [], header)).rejects.toThrow(
      "dryRun requires exactly 3 CSV rows, got 0",
    );
  });

  it("does not call createPyodideWorker when row count is wrong", async () => {
    await dryRun(script, ["a"], header).catch(() => {});
    expect(mockInit).not.toHaveBeenCalled();
    expect(mockTerminate).not.toHaveBeenCalled();
  });

  it("returns DryRunResult with 3 rows on success", async () => {
    const result = await dryRun(script, rows, header);

    expect(result.rows).toHaveLength(3);
    expect(result.rows[0].input).toBe("name,age\nAlice,30");
    expect(result.rows[1].input).toBe("name,age\nBob,25");
    expect(result.rows[2].input).toBe("name,age\nCarol,40");
    expect(typeof result.totalDurationMs).toBe("number");
  });

  it("populates output and durationMs from exec results", async () => {
    mockExec
      .mockResolvedValueOnce({ stdout: "out1", stderr: "", durationMs: 5 })
      .mockResolvedValueOnce({ stdout: "out2", stderr: "", durationMs: 15 })
      .mockResolvedValueOnce({ stdout: "out3", stderr: "", durationMs: 25 });

    const result = await dryRun(script, rows, header);

    expect(result.rows[0].output).toBe("out1");
    expect(result.rows[0].durationMs).toBe(5);
    expect(result.rows[1].output).toBe("out2");
    expect(result.rows[1].durationMs).toBe(15);
    expect(result.rows[2].output).toBe("out3");
    expect(result.rows[2].durationMs).toBe(25);
  });

  it("calls init before any exec", async () => {
    const callOrder: string[] = [];
    mockInit.mockImplementation(async () => {
      callOrder.push("init");
    });
    mockExec.mockImplementation(async () => {
      callOrder.push("exec");
      return { stdout: "", stderr: "", durationMs: 0 };
    });

    await dryRun(script, rows, header);

    expect(callOrder[0]).toBe("init");
    expect(callOrder.filter((c) => c === "exec")).toHaveLength(3);
  });

  it("calls exec sequentially, 3 times, with correct stdinData", async () => {
    await dryRun(script, rows, header);

    expect(mockExec).toHaveBeenCalledTimes(3);
    expect(mockExec).toHaveBeenNthCalledWith(1, script, "name,age\nAlice,30");
    expect(mockExec).toHaveBeenNthCalledWith(2, script, "name,age\nBob,25");
    expect(mockExec).toHaveBeenNthCalledWith(3, script, "name,age\nCarol,40");
  });

  // ---- Error handling -----------------------------------------------------

  it("stops on first row error and does not execute remaining rows", async () => {
    mockExec
      .mockResolvedValueOnce({ stdout: "ok", stderr: "", durationMs: 5 })
      .mockRejectedValueOnce(new Error("Script failed on row 2"));

    await expect(dryRun(script, rows, header)).rejects.toThrow(
      "Script failed on row 2",
    );

    // Only 2 exec calls should have been made (row 1 succeeded, row 2 failed, row 3 skipped)
    expect(mockExec).toHaveBeenCalledTimes(2);
  });

  it("stops on first row error (row 1 fails)", async () => {
    mockExec.mockRejectedValueOnce(new Error("Row 1 boom"));

    await expect(dryRun(script, rows, header)).rejects.toThrow("Row 1 boom");

    expect(mockExec).toHaveBeenCalledTimes(1);
  });

  it("terminates worker even on exec error (finally block)", async () => {
    mockExec.mockRejectedValue(new Error("exec failed"));

    await dryRun(script, rows, header).catch(() => {});

    expect(mockTerminate).toHaveBeenCalledOnce();
  });

  it("terminates worker on success", async () => {
    await dryRun(script, rows, header);

    expect(mockTerminate).toHaveBeenCalledOnce();
  });

  it("fails if worker init fails", async () => {
    mockInit.mockRejectedValue(new Error("Pyodide load failed"));

    await expect(dryRun(script, rows, header)).rejects.toThrow(
      "Pyodide load failed",
    );

    // No exec calls should have been made
    expect(mockExec).not.toHaveBeenCalled();
  });

  it("terminates worker even when init fails (finally block)", async () => {
    mockInit.mockRejectedValue(new Error("init boom"));

    await dryRun(script, rows, header).catch(() => {});

    expect(mockTerminate).toHaveBeenCalledOnce();
  });

  it("propagates timeout error, stops remaining rows, and terminates worker", async () => {
    mockExec
      .mockResolvedValueOnce({ stdout: "ok", stderr: "", durationMs: 5 })
      .mockRejectedValueOnce(new Error("Execution timed out (30s)"));

    await expect(dryRun(script, rows, header)).rejects.toThrow(
      "Execution timed out (30s)",
    );

    expect(mockExec).toHaveBeenCalledTimes(2);
    expect(mockTerminate).toHaveBeenCalledOnce();
  });
});
