import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { mount, flushPromises } from "@vue/test-utils";
import type { TaskDispatch, TaskStats } from "../../../../integrations/ui/types";
import type { SubmitTaskRequest } from "../api";

// ---------------------------------------------------------------------------
// Hoist mocks above imports. VolunteerRunner imports:
//   - `pollNextTask`, `submitTask`, `getTaskStats` from "./api"   → "../api"
//   - `createPyodideWorker` from "../../pyodide-runtime/ui/pyodideWorker"
// We intercept both so no real network or Web Worker is touched.
// ---------------------------------------------------------------------------

const {
  mockPollNextTask,
  mockSubmitTask,
  mockGetTaskStats,
  mockStartJob,
  mockCreatePyodideWorker,
  mockInit,
  mockExec,
  mockTerminate,
} = vi.hoisted(() => {
  const mockInit = vi.fn<() => Promise<void>>();
  const mockExec =
    vi.fn<
      (script: string, argv: string[]) => Promise<{
        stdout: string;
        stderr: string;
        durationMs: number;
      }>
    >();
  const mockTerminate = vi.fn<() => void>();
  const mockCreatePyodideWorker = vi.fn(() => ({
    init: mockInit,
    exec: mockExec,
    terminate: mockTerminate,
  }));
  return {
    mockPollNextTask:
      vi.fn<(workerId: string) => Promise<TaskDispatch | null>>(),
    mockSubmitTask:
      vi.fn<(taskId: string, req: SubmitTaskRequest) => Promise<void>>(),
    mockGetTaskStats:
      vi.fn<(jobId: string, workerId: string) => Promise<TaskStats>>(),
    mockStartJob: vi.fn(),
    mockCreatePyodideWorker,
    mockInit,
    mockExec,
    mockTerminate,
  };
});

vi.mock("../api", () => ({
  pollNextTask: mockPollNextTask,
  submitTask: mockSubmitTask,
  getTaskStats: mockGetTaskStats,
  startJob: mockStartJob,
}));

vi.mock("../../../pyodide-runtime/ui/pyodideWorker", () => ({
  createPyodideWorker: mockCreatePyodideWorker,
}));

// Import the component AFTER the mocks are set up.
import VolunteerRunner from "../VolunteerRunner.vue";

// ---------------------------------------------------------------------------
// Fixtures / helpers
// ---------------------------------------------------------------------------

const WORKER_ID_KEY = "foinc.worker_id";

function makeDispatch(overrides: Partial<TaskDispatch> = {}): TaskDispatch {
  return {
    task_id: "task-1234abcd-0000-0000-0000-000000000000",
    job_id: "job-5678efef-0000-0000-0000-000000000000",
    script: "import sys\nprint(sys.argv)",
    input_rows: ["42,7"],
    deadline_at: "2026-04-17T10:00:00Z",
    ...overrides,
  };
}

function makeStats(overrides: Partial<TaskStats> = {}): TaskStats {
  return {
    pending: 0,
    in_flight: 0,
    awaiting_consensus: 0,
    completed_total: 0,
    completed_by_me: 0,
    ...overrides,
  };
}

/** Resolve all pending microtasks repeatedly so chained awaits settle. */
async function settle(times = 8) {
  for (let i = 0; i < times; i++) {
    await Promise.resolve();
    await flushPromises();
  }
}

// ---------------------------------------------------------------------------
// Global mock setup that applies to every test
// ---------------------------------------------------------------------------

beforeEach(() => {
  vi.useFakeTimers();

  mockPollNextTask.mockReset();
  mockSubmitTask.mockReset();
  mockGetTaskStats.mockReset();
  mockCreatePyodideWorker.mockClear();
  mockInit.mockReset();
  mockExec.mockReset();
  mockTerminate.mockReset();

  // Quiet defaults — individual tests override as needed.
  mockPollNextTask.mockResolvedValue(null);
  mockSubmitTask.mockResolvedValue(undefined);
  mockGetTaskStats.mockResolvedValue(makeStats());
  mockInit.mockResolvedValue(undefined);
  mockExec.mockResolvedValue({
    stdout: "ok",
    stderr: "",
    durationMs: 11,
  });

  // crypto.randomUUID exists in modern jsdom but we stub for determinism.
  if (!globalThis.crypto) {
    Object.defineProperty(globalThis, "crypto", {
      value: { randomUUID: () => "fresh-uuid-0000" },
      configurable: true,
    });
  }
  const cryptoObj = globalThis.crypto as Crypto & { randomUUID: () => string };
  vi.spyOn(cryptoObj, "randomUUID").mockReturnValue(
    "fresh-uuid-0000-0000-0000-0000-0000",
  );

  localStorage.clear();
});

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

// ---------------------------------------------------------------------------
// Tests — worker_id lifecycle
// ---------------------------------------------------------------------------

describe("VolunteerRunner.vue — worker_id", () => {
  it("reads_existing_worker_id_from_localStorage_on_mount", async () => {
    localStorage.setItem(WORKER_ID_KEY, "persisted-worker-id-abcdef");

    const wrapper = mount(VolunteerRunner);
    // Flush onMounted microtasks.
    await settle();

    // First poll fires immediately (delay 0); it should use the stored id.
    await vi.advanceTimersByTimeAsync(1);
    await settle();

    expect(mockPollNextTask).toHaveBeenCalled();
    expect(mockPollNextTask.mock.calls[0][0]).toBe(
      "persisted-worker-id-abcdef",
    );

    // And the stored id is NOT overwritten.
    expect(localStorage.getItem(WORKER_ID_KEY)).toBe(
      "persisted-worker-id-abcdef",
    );
    wrapper.unmount();
  });

  it("generates_and_persists_a_fresh_worker_id_when_missing", async () => {
    expect(localStorage.getItem(WORKER_ID_KEY)).toBeNull();

    const wrapper = mount(VolunteerRunner);
    await settle();

    // crypto.randomUUID was used to mint one.
    const stored = localStorage.getItem(WORKER_ID_KEY);
    expect(stored).toBe("fresh-uuid-0000-0000-0000-0000-0000");

    await vi.advanceTimersByTimeAsync(1);
    await settle();
    expect(mockPollNextTask).toHaveBeenCalledWith(stored);
    wrapper.unmount();
  });
});

// ---------------------------------------------------------------------------
// Tests — polling cadence
// ---------------------------------------------------------------------------

describe("VolunteerRunner.vue — polling cadence", () => {
  it("re_polls_pollNextTask_after_the_poll_interval_elapses", async () => {
    localStorage.setItem(WORKER_ID_KEY, "worker-1");
    mockPollNextTask.mockResolvedValue(null);

    const wrapper = mount(VolunteerRunner);
    await settle();

    // First poll (scheduled at 0) fires.
    await vi.advanceTimersByTimeAsync(1);
    await settle();
    expect(mockPollNextTask).toHaveBeenCalledTimes(1);

    // Advance another 2000ms — the interval — to fire the follow-up poll.
    await vi.advanceTimersByTimeAsync(2000);
    await settle();
    expect(mockPollNextTask).toHaveBeenCalledTimes(2);

    // And again.
    await vi.advanceTimersByTimeAsync(2000);
    await settle();
    expect(mockPollNextTask).toHaveBeenCalledTimes(3);

    wrapper.unmount();
  });

  it("continues_polling_when_pollNextTask_returns_null", async () => {
    localStorage.setItem(WORKER_ID_KEY, "worker-1");
    mockPollNextTask.mockResolvedValue(null);

    const wrapper = mount(VolunteerRunner);
    await settle();
    await vi.advanceTimersByTimeAsync(1);
    await settle();

    // Advance several intervals; pollNextTask should be called every time
    // (no dispatch ever means the runner stays idle and keeps polling).
    for (let i = 0; i < 3; i++) {
      await vi.advanceTimersByTimeAsync(2000);
      await settle();
    }
    expect(mockPollNextTask).toHaveBeenCalledTimes(1 + 3);
    expect(mockCreatePyodideWorker).not.toHaveBeenCalled();
    expect(mockSubmitTask).not.toHaveBeenCalled();

    wrapper.unmount();
  });
});

// ---------------------------------------------------------------------------
// Tests — happy-path: dispatch → exec → submit → resume polling
// ---------------------------------------------------------------------------

describe("VolunteerRunner.vue — dispatch and execution", () => {
  it("spawns_PyodideWorker_calls_exec_then_submit_and_clears_currentTask", async () => {
    localStorage.setItem(WORKER_ID_KEY, "worker-1");
    const dispatch = makeDispatch();

    // First poll returns a dispatch, subsequent polls return null so the
    // timers keep firing quietly.
    mockPollNextTask
      .mockResolvedValueOnce(dispatch)
      .mockResolvedValue(null);

    const wrapper = mount(VolunteerRunner);
    await settle();
    await vi.advanceTimersByTimeAsync(1);
    await settle();

    // Pyodide worker created exactly once for this dispatch.
    expect(mockCreatePyodideWorker).toHaveBeenCalledTimes(1);
    expect(mockInit).toHaveBeenCalledTimes(1);

    // exec called with the task script and the first input row split on ","
    expect(mockExec).toHaveBeenCalledTimes(1);
    expect(mockExec).toHaveBeenCalledWith(dispatch.script, ["42", "7"]);

    // submitTask called with the exec result forwarded as-is.
    expect(mockSubmitTask).toHaveBeenCalledTimes(1);
    const [taskId, req] = mockSubmitTask.mock.calls[0];
    expect(taskId).toBe(dispatch.task_id);
    expect(req).toEqual({
      worker_id: "worker-1",
      stdout: "ok",
      stderr: "",
      duration_ms: 11,
    });

    // Spec: worker is reused. After a successful dispatch the long-lived
    // runner stays alive; terminate() is not called until unmount.
    expect(mockTerminate).toHaveBeenCalledTimes(0);

    // Template is back to "Idle" because currentTask is cleared.
    expect(wrapper.text()).toContain("Idle");

    // After completing, polling resumes on the next interval.
    await vi.advanceTimersByTimeAsync(2000);
    await settle();
    expect(mockPollNextTask).toHaveBeenCalledTimes(2);

    // Now unmount and confirm the unmount-path terminates the reused worker.
    wrapper.unmount();
    expect(mockTerminate).toHaveBeenCalledTimes(1);
  });

  it("does_NOT_submit_when_exec_rejects_continues_polling_silently", async () => {
    localStorage.setItem(WORKER_ID_KEY, "w");
    const dispatch = makeDispatch();

    mockPollNextTask
      .mockResolvedValueOnce(dispatch)
      .mockResolvedValue(null);
    // "script crashed" is not one of the two dead-worker signals, so per
    // spec's "default is reuse" rule this is a user-script fault — the
    // worker stays alive, no terminate, no notify.
    mockExec.mockRejectedValueOnce(new Error("script crashed"));

    const wrapper = mount(VolunteerRunner);
    await settle();
    await vi.advanceTimersByTimeAsync(1);
    await settle();

    // Worker was created and we attempted to exec.
    expect(mockExec).toHaveBeenCalledTimes(1);

    // Crucially: submitTask was NOT called (silent reclamation).
    expect(mockSubmitTask).not.toHaveBeenCalled();

    // Spec: user-script fault → reuse. terminate() is NOT called on the
    // reject path; the runner stays alive for the next dispatch.
    expect(mockTerminate).toHaveBeenCalledTimes(0);

    // No notify emitted (spec: per-task exec rejections are silent).
    expect(wrapper.emitted("notify")).toBeUndefined();

    // Polling resumes.
    await vi.advanceTimersByTimeAsync(2000);
    await settle();
    expect(mockPollNextTask).toHaveBeenCalledTimes(2);

    wrapper.unmount();
  });

  it("does_NOT_submit_when_init_rejects", async () => {
    localStorage.setItem(WORKER_ID_KEY, "w");
    mockPollNextTask
      .mockResolvedValueOnce(makeDispatch())
      .mockResolvedValue(null);
    mockInit.mockRejectedValueOnce(new Error("pyodide failed to load"));

    const wrapper = mount(VolunteerRunner);
    await settle();
    await vi.advanceTimersByTimeAsync(1);
    await settle();

    expect(mockExec).not.toHaveBeenCalled();
    expect(mockSubmitTask).not.toHaveBeenCalled();
    // Spec Lazy-create failure branch: best-effort terminate of the failed
    // Web Worker process, then runner = null.
    expect(mockTerminate).toHaveBeenCalledTimes(1);

    // Spec mandates emit notify at level: "error" on the first init()
    // failure after a Lazy-create.
    const notifyEvents = wrapper.emitted("notify");
    expect(notifyEvents).toBeDefined();
    expect(notifyEvents).toHaveLength(1);
    const [toast] = notifyEvents![0] as [{ level: string; message: string }];
    expect(toast.level).toBe("error");
    expect(toast.message).toBe("pyodide failed to load");

    wrapper.unmount();
  });
});

// ---------------------------------------------------------------------------
// Tests — Pyodide worker lifecycle (reuse, crash recovery, unmount guard)
// ---------------------------------------------------------------------------

describe("VolunteerRunner.vue — Pyodide worker lifecycle", () => {
  it("reuses_the_same_worker_across_two_successful_dispatches", async () => {
    localStorage.setItem(WORKER_ID_KEY, "w");
    const d1 = makeDispatch({
      task_id: "aaaa1111-0000-0000-0000-000000000000",
    });
    const d2 = makeDispatch({
      task_id: "bbbb2222-0000-0000-0000-000000000000",
    });

    mockPollNextTask
      .mockResolvedValueOnce(d1)
      .mockResolvedValueOnce(d2)
      .mockResolvedValue(null);

    const wrapper = mount(VolunteerRunner);
    await settle();
    // First poll (delay 0) → dispatch 1.
    await vi.advanceTimersByTimeAsync(1);
    await settle();
    // Second poll (after POLL_INTERVAL_MS) → dispatch 2.
    await vi.advanceTimersByTimeAsync(2000);
    await settle();

    // Spec: single long-lived runner; Lazy-create runs once, Reuse on 2nd.
    expect(mockCreatePyodideWorker).toHaveBeenCalledTimes(1);
    expect(mockInit).toHaveBeenCalledTimes(1);
    expect(mockExec).toHaveBeenCalledTimes(2);

    // Both dispatches submitted results.
    expect(mockSubmitTask).toHaveBeenCalledTimes(2);
    expect(mockSubmitTask.mock.calls[0][0]).toBe(d1.task_id);
    expect(mockSubmitTask.mock.calls[1][0]).toBe(d2.task_id);

    // No terminate during reuse.
    expect(mockTerminate).toHaveBeenCalledTimes(0);

    // Unmount terminates the reused worker exactly once.
    wrapper.unmount();
    expect(mockTerminate).toHaveBeenCalledTimes(1);
  });

  it("reuses_worker_after_user_script_fault_without_recreate", async () => {
    localStorage.setItem(WORKER_ID_KEY, "w");
    const d1 = makeDispatch({
      task_id: "aaaa1111-0000-0000-0000-000000000000",
    });
    const d2 = makeDispatch({
      task_id: "bbbb2222-0000-0000-0000-000000000000",
    });

    mockPollNextTask
      .mockResolvedValueOnce(d1)
      .mockResolvedValueOnce(d2)
      .mockResolvedValue(null);

    // First exec: user-script fault (a Python runtime error). Per spec's
    // "default is reuse" rule, the worker stays alive.
    mockExec
      .mockRejectedValueOnce(new Error("RuntimeError: boom"))
      .mockResolvedValueOnce({
        stdout: "ok2",
        stderr: "",
        durationMs: 22,
      });

    const wrapper = mount(VolunteerRunner);
    await settle();
    await vi.advanceTimersByTimeAsync(1);
    await settle();
    await vi.advanceTimersByTimeAsync(2000);
    await settle();

    // No recreate — one worker, one init.
    expect(mockCreatePyodideWorker).toHaveBeenCalledTimes(1);
    expect(mockInit).toHaveBeenCalledTimes(1);
    expect(mockExec).toHaveBeenCalledTimes(2);

    // Only the second dispatch submitted; the first was silently reclaimed
    // by the backend via deadline.
    expect(mockSubmitTask).toHaveBeenCalledTimes(1);
    expect(mockSubmitTask.mock.calls[0][0]).toBe(d2.task_id);

    // No terminate during reuse; no notify for user-script fault.
    expect(mockTerminate).toHaveBeenCalledTimes(0);
    expect(wrapper.emitted("notify")).toBeUndefined();

    wrapper.unmount();
  });

  it("recreates_worker_after_30s_timeout_dead_worker_signal", async () => {
    localStorage.setItem(WORKER_ID_KEY, "w");
    const d1 = makeDispatch({
      task_id: "aaaa1111-0000-0000-0000-000000000000",
    });
    const d2 = makeDispatch({
      task_id: "bbbb2222-0000-0000-0000-000000000000",
    });

    mockPollNextTask
      .mockResolvedValueOnce(d1)
      .mockResolvedValueOnce(d2)
      .mockResolvedValue(null);

    // Dead-worker signal #1: 30s timeout.
    mockExec
      .mockRejectedValueOnce(new Error("Execution timed out (30s)"))
      .mockResolvedValueOnce({
        stdout: "ok2",
        stderr: "",
        durationMs: 22,
      });

    const wrapper = mount(VolunteerRunner);
    await settle();
    await vi.advanceTimersByTimeAsync(1);
    await settle();
    await vi.advanceTimersByTimeAsync(2000);
    await settle();

    // Spec: Crash recovery — terminate dead worker, next dispatch
    // lazy-creates a fresh one.
    expect(mockCreatePyodideWorker).toHaveBeenCalledTimes(2);
    expect(mockInit).toHaveBeenCalledTimes(2);
    // Before unmount: exactly one terminate from the crash-recovery path.
    expect(mockTerminate).toHaveBeenCalledTimes(1);

    // First submit not called (backend reclaims); second submit called.
    expect(mockSubmitTask).toHaveBeenCalledTimes(1);
    expect(mockSubmitTask.mock.calls[0][0]).toBe(d2.task_id);

    // Spec: crash recovery is silent — no notify.
    expect(wrapper.emitted("notify")).toBeUndefined();

    wrapper.unmount();
  });

  it("recreates_worker_after_worker_is_terminated_dead_worker_signal", async () => {
    localStorage.setItem(WORKER_ID_KEY, "w");
    const d1 = makeDispatch({
      task_id: "aaaa1111-0000-0000-0000-000000000000",
    });
    const d2 = makeDispatch({
      task_id: "bbbb2222-0000-0000-0000-000000000000",
    });

    mockPollNextTask
      .mockResolvedValueOnce(d1)
      .mockResolvedValueOnce(d2)
      .mockResolvedValue(null);

    // Dead-worker signal #2: immediate reject from post-Terminated state.
    mockExec
      .mockRejectedValueOnce(new Error("Worker is terminated"))
      .mockResolvedValueOnce({
        stdout: "ok2",
        stderr: "",
        durationMs: 22,
      });

    const wrapper = mount(VolunteerRunner);
    await settle();
    await vi.advanceTimersByTimeAsync(1);
    await settle();
    await vi.advanceTimersByTimeAsync(2000);
    await settle();

    expect(mockCreatePyodideWorker).toHaveBeenCalledTimes(2);
    expect(mockInit).toHaveBeenCalledTimes(2);
    // Before unmount: exactly one terminate from the crash-recovery path.
    expect(mockTerminate).toHaveBeenCalledTimes(1);

    expect(mockSubmitTask).toHaveBeenCalledTimes(1);
    expect(mockSubmitTask.mock.calls[0][0]).toBe(d2.task_id);

    expect(wrapper.emitted("notify")).toBeUndefined();

    wrapper.unmount();
  });

  it("recreates_worker_when_timeout_error_message_is_a_superset", async () => {
    // Pins `.includes()` semantics in isDeadWorkerRejection. A future
    // refactor to strict equality (`=== "Execution timed out (30s)"`)
    // would break this test because the real message is wrapped inside
    // a longer "TaskTimeout: ... at line 42" string.
    localStorage.setItem(WORKER_ID_KEY, "w");
    const d1 = makeDispatch({
      task_id: "aaaa1111-0000-0000-0000-000000000000",
    });
    const d2 = makeDispatch({
      task_id: "bbbb2222-0000-0000-0000-000000000000",
    });

    mockPollNextTask
      .mockResolvedValueOnce(d1)
      .mockResolvedValueOnce(d2)
      .mockResolvedValue(null);

    // Superset of the dead-worker signal: message contains the exact
    // "Execution timed out (30s)" substring surrounded by prefix/suffix.
    mockExec
      .mockRejectedValueOnce(
        new Error("TaskTimeout: Execution timed out (30s) at line 42"),
      )
      .mockResolvedValueOnce({
        stdout: "ok2",
        stderr: "",
        durationMs: 22,
      });

    const wrapper = mount(VolunteerRunner);
    await settle();
    await vi.advanceTimersByTimeAsync(1);
    await settle();
    await vi.advanceTimersByTimeAsync(2000);
    await settle();

    // Crash recovery still triggers because classifier uses includes().
    expect(mockCreatePyodideWorker).toHaveBeenCalledTimes(2);
    expect(mockInit).toHaveBeenCalledTimes(2);

    wrapper.unmount();
  });

  it("unmount_during_init_guards_exec_submit_and_notify", async () => {
    // Covers the post-init mounted guard (VolunteerRunner.vue lines 159-162):
    // if the component is unmounted while init() is in flight, neither
    // exec() nor submitTask() nor notify should be touched afterwards.
    localStorage.setItem(WORKER_ID_KEY, "w");
    const dispatch = makeDispatch();

    mockPollNextTask
      .mockResolvedValueOnce(dispatch)
      .mockResolvedValue(null);

    // Manually resolvable init so we can unmount BEFORE init resolves.
    let resolveInit: () => void;
    const pendingInit = new Promise<void>((r) => {
      resolveInit = r;
    });
    mockInit.mockReturnValueOnce(pendingInit);

    const wrapper = mount(VolunteerRunner);
    await settle();
    await vi.advanceTimersByTimeAsync(1);
    await settle();

    // init is in flight; exec should not have been called yet.
    expect(mockCreatePyodideWorker).toHaveBeenCalledTimes(1);
    expect(mockInit).toHaveBeenCalledTimes(1);
    expect(mockExec).not.toHaveBeenCalled();

    wrapper.unmount();
    // Unmount itself terminates the runner exactly once.
    expect(mockTerminate).toHaveBeenCalledTimes(1);

    // Now let init resolve AFTER unmount. The post-init `!mounted || runner === null`
    // guard must bail before reaching exec.
    resolveInit!();
    await settle();

    // Post-init guard: exec never called, no submit, no notify, no extra terminate.
    expect(mockExec).not.toHaveBeenCalled();
    expect(mockSubmitTask).not.toHaveBeenCalled();
    expect(wrapper.emitted("notify")).toBeUndefined();
    expect(mockTerminate).toHaveBeenCalledTimes(1);
  });

  it("unmount_during_exec_guards_submit_and_notify_with_mounted_flag", async () => {
    localStorage.setItem(WORKER_ID_KEY, "w");
    const dispatch = makeDispatch();

    mockPollNextTask
      .mockResolvedValueOnce(dispatch)
      .mockResolvedValue(null);

    // Manually resolvable exec so we can unmount BEFORE the exec resolves.
    let resolveExec: (v: {
      stdout: string;
      stderr: string;
      durationMs: number;
    }) => void;
    const pendingExec = new Promise<{
      stdout: string;
      stderr: string;
      durationMs: number;
    }>((r) => {
      resolveExec = r;
    });
    mockExec.mockReturnValueOnce(pendingExec);

    const wrapper = mount(VolunteerRunner);
    await settle();
    await vi.advanceTimersByTimeAsync(1);
    await settle();

    // exec is in flight; unmount now.
    expect(mockExec).toHaveBeenCalledTimes(1);
    expect(mockSubmitTask).not.toHaveBeenCalled();

    wrapper.unmount();
    // Unmount terminates the reused worker exactly once.
    expect(mockTerminate).toHaveBeenCalledTimes(1);

    // Snapshot getTaskStats call count before the late resolve so we can
    // assert the finally { if (mounted) refreshStats() } guard suppresses
    // a post-unmount stats refresh. Catches removal of that guard.
    const statsCallsBeforeLateResolve = mockGetTaskStats.mock.calls.length;

    // Now let exec resolve AFTER unmount.
    resolveExec!({ stdout: "late", stderr: "", durationMs: 9 });
    await settle();

    // mounted-flag guard: no submit, no notify, no extra terminate.
    expect(mockSubmitTask).not.toHaveBeenCalled();
    expect(wrapper.emitted("notify")).toBeUndefined();
    expect(mockTerminate).toHaveBeenCalledTimes(1);
    // And the finally-block's `if (mounted) refreshStats()` guard must
    // suppress a post-unmount stats fetch.
    expect(mockGetTaskStats.mock.calls.length).toBe(
      statsCallsBeforeLateResolve,
    );
  });
});

// ---------------------------------------------------------------------------
// Tests — getTaskStats polling
// ---------------------------------------------------------------------------

describe("VolunteerRunner.vue — stats polling", () => {
  it("does_not_call_getTaskStats_before_any_job_is_seen", async () => {
    localStorage.setItem(WORKER_ID_KEY, "w");
    mockPollNextTask.mockResolvedValue(null);

    const wrapper = mount(VolunteerRunner);
    await settle();

    // Advance several stats intervals with no task ever picked up.
    for (let i = 0; i < 3; i++) {
      await vi.advanceTimersByTimeAsync(2000);
      await settle();
    }
    expect(mockGetTaskStats).not.toHaveBeenCalled();

    wrapper.unmount();
  });

  it("calls_getTaskStats_after_a_task_was_picked_up_setting_activeJobId", async () => {
    localStorage.setItem(WORKER_ID_KEY, "w");
    const dispatch = makeDispatch();
    mockPollNextTask
      .mockResolvedValueOnce(dispatch)
      .mockResolvedValue(null);

    const wrapper = mount(VolunteerRunner);
    await settle();
    await vi.advanceTimersByTimeAsync(1);
    await settle();

    // The finally-block in runTask calls refreshStats() eagerly, so
    // getTaskStats runs immediately with the dispatch's job_id.
    expect(mockGetTaskStats).toHaveBeenCalled();
    const firstCall = mockGetTaskStats.mock.calls[0];
    expect(firstCall[0]).toBe(dispatch.job_id);
    expect(firstCall[1]).toBe("w");

    wrapper.unmount();
  });
});

// ---------------------------------------------------------------------------
// Tests — rendering
// ---------------------------------------------------------------------------

describe("VolunteerRunner.vue — rendering", () => {
  it("renders_idle_message_when_no_task_and_no_stats", async () => {
    localStorage.setItem(WORKER_ID_KEY, "w");
    mockPollNextTask.mockResolvedValue(null);

    const wrapper = mount(VolunteerRunner);
    await settle();

    expect(wrapper.text()).toContain("Idle");
    expect(wrapper.text()).toContain("waiting for work");
    // No stats cards when stats haven't been fetched yet.
    expect(wrapper.find(".stats").exists()).toBe(false);

    wrapper.unmount();
  });

  it("renders_running_section_while_task_is_in_flight", async () => {
    localStorage.setItem(WORKER_ID_KEY, "w");
    const dispatch = makeDispatch({
      task_id: "deadbeef-1111-2222-3333-444444444444",
    });

    // Keep exec pending forever so we can observe the mid-task UI.
    mockPollNextTask
      .mockResolvedValueOnce(dispatch)
      .mockResolvedValue(null);
    mockExec.mockReturnValue(new Promise(() => { /* never resolves */ }));

    const wrapper = mount(VolunteerRunner);
    await settle();
    await vi.advanceTimersByTimeAsync(1);
    await settle();

    // While the exec promise hangs, currentTask is set and the "Running"
    // line includes the short task id (first 8 chars).
    const text = wrapper.text();
    expect(text).toContain("Running task");
    expect(text).toContain("deadbeef");
    expect(text).not.toContain("Idle");

    wrapper.unmount();
  });

  it("renders_stat_cards_with_counts_once_stats_load", async () => {
    localStorage.setItem(WORKER_ID_KEY, "w");
    const dispatch = makeDispatch();
    mockPollNextTask
      .mockResolvedValueOnce(dispatch)
      .mockResolvedValue(null);
    mockGetTaskStats.mockResolvedValue(
      makeStats({
        pending: 3,
        in_flight: 4,
        completed_total: 5,
        completed_by_me: 6,
      }),
    );

    const wrapper = mount(VolunteerRunner);
    await settle();
    await vi.advanceTimersByTimeAsync(1);
    await settle();
    // Let the refreshStats fetch resolve.
    await settle();

    const text = wrapper.text();
    // Labels:
    expect(text).toContain("Pending");
    expect(text).toContain("In flight");
    expect(text).toContain("Completed (total)");
    expect(text).toContain("Completed (you)");
    // Values rendered as numbers:
    const values = wrapper.findAll(".stat__value").map((el) => el.text());
    expect(values).toEqual(["3", "4", "5", "6"]);

    wrapper.unmount();
  });
});

// ---------------------------------------------------------------------------
// Tests — timer cleanup on unmount
// ---------------------------------------------------------------------------

describe("VolunteerRunner.vue — unmount cleanup", () => {
  it("does_not_fire_additional_polls_after_unmount", async () => {
    localStorage.setItem(WORKER_ID_KEY, "w");
    mockPollNextTask.mockResolvedValue(null);

    const wrapper = mount(VolunteerRunner);
    await settle();
    await vi.advanceTimersByTimeAsync(1);
    await settle();

    const callsAtUnmount = mockPollNextTask.mock.calls.length;

    wrapper.unmount();

    // Advance well past several poll intervals — no more calls.
    await vi.advanceTimersByTimeAsync(60_000);
    await settle();

    expect(mockPollNextTask.mock.calls.length).toBe(callsAtUnmount);
  });

  it("does_not_fire_additional_stats_calls_after_unmount", async () => {
    localStorage.setItem(WORKER_ID_KEY, "w");
    const dispatch = makeDispatch();
    mockPollNextTask
      .mockResolvedValueOnce(dispatch)
      .mockResolvedValue(null);

    const wrapper = mount(VolunteerRunner);
    await settle();
    await vi.advanceTimersByTimeAsync(1);
    await settle();

    const callsAtUnmount = mockGetTaskStats.mock.calls.length;

    wrapper.unmount();

    await vi.advanceTimersByTimeAsync(60_000);
    await settle();

    expect(mockGetTaskStats.mock.calls.length).toBe(callsAtUnmount);
  });
});
