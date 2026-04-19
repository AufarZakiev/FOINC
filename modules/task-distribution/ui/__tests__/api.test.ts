import { describe, it, expect, vi, afterEach } from "vitest";
import {
  getTaskStats,
  pollNextTask,
  startJob,
  submitTask,
} from "../api";
import type { SubmitTaskRequest } from "../api";

/**
 * Build a minimal Response-like object that satisfies the subset of the fetch
 * API surface the api layer uses: `status`, `ok`, and `json()`. We avoid the
 * real `Response` constructor so we can drive arbitrary status codes
 * (including 204 with no body) without fighting the DOM `Response` semantics
 * in jsdom. Same pattern as `modules/upload/ui/__tests__/api.test.ts`.
 */
function fakeResponse(init: {
  status: number;
  body?: unknown;
  jsonThrows?: boolean;
}): Response {
  return {
    status: init.status,
    ok: init.status >= 200 && init.status < 300,
    json: () =>
      init.jsonThrows
        ? Promise.reject(new Error("invalid json"))
        : Promise.resolve(init.body),
  } as unknown as Response;
}

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

// -------------------------------------------------------------------------
// startJob
// -------------------------------------------------------------------------

describe("startJob", () => {
  it("posts_to_start_endpoint_and_returns_parsed_body_on_200", async () => {
    const body = { job_id: "job-xyz", task_count: 7 };
    const fetchMock = vi
      .fn()
      .mockResolvedValue(fakeResponse({ status: 200, body }));
    vi.stubGlobal("fetch", fetchMock);

    const result = await startJob("job-xyz");
    expect(result).toEqual(body);

    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe("/api/jobs/job-xyz/start");
    expect((init as RequestInit).method).toBe("POST");
    expect((init as RequestInit).headers).toMatchObject({
      "Content-Type": "application/json",
    });
  });

  it("serializes_chunk_size_when_provided", async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      fakeResponse({ status: 200, body: { job_id: "j", task_count: 1 } }),
    );
    vi.stubGlobal("fetch", fetchMock);

    await startJob("j", 42);
    const body = JSON.parse((fetchMock.mock.calls[0][1] as RequestInit).body as string);
    expect(body).toEqual({ chunk_size: 42 });
  });

  it("serializes_chunk_size_null_when_omitted", async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      fakeResponse({ status: 200, body: { job_id: "j", task_count: 1 } }),
    );
    vi.stubGlobal("fetch", fetchMock);

    await startJob("j");
    const body = JSON.parse((fetchMock.mock.calls[0][1] as RequestInit).body as string);
    expect(body).toEqual({ chunk_size: null });
  });

  it("throws_with_message_from_body_on_404", async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      fakeResponse({ status: 404, body: { error: "Job not found" } }),
    );
    vi.stubGlobal("fetch", fetchMock);

    await expect(startJob("missing")).rejects.toThrow("Job not found");
  });

  it("throws_on_409_conflict", async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      fakeResponse({
        status: 409,
        body: { error: "Job is not in uploaded state" },
      }),
    );
    vi.stubGlobal("fetch", fetchMock);

    await expect(startJob("busy")).rejects.toThrow(
      "Job is not in uploaded state",
    );
  });

  it("throws_with_status_suffix_when_body_has_no_error_field", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValue(fakeResponse({ status: 500, body: { detail: "x" } }));
    vi.stubGlobal("fetch", fetchMock);

    await expect(startJob("boom")).rejects.toThrow(
      "Start job failed with status 500",
    );
  });

  it("throws_with_fallback_message_when_body_is_unparseable", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValue(fakeResponse({ status: 500, jsonThrows: true }));
    vi.stubGlobal("fetch", fetchMock);

    await expect(startJob("boom")).rejects.toThrow("Start job failed");
  });
});

// -------------------------------------------------------------------------
// pollNextTask
// -------------------------------------------------------------------------

describe("pollNextTask", () => {
  it("resolves_null_on_204_no_content", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValue(fakeResponse({ status: 204, jsonThrows: true }));
    vi.stubGlobal("fetch", fetchMock);

    await expect(pollNextTask("worker-1")).resolves.toBeNull();

    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe("/api/tasks/next");
    expect((init as RequestInit).method).toBe("POST");
    const body = JSON.parse((init as RequestInit).body as string);
    expect(body).toEqual({ worker_id: "worker-1" });
  });

  it("returns_parsed_dispatch_on_200", async () => {
    const dispatch = {
      task_id: "t1",
      job_id: "j1",
      script: "print('x')",
      input_rows: ["1,2"],
      deadline_at: "2026-04-17T10:00:00Z",
    };
    const fetchMock = vi
      .fn()
      .mockResolvedValue(fakeResponse({ status: 200, body: dispatch }));
    vi.stubGlobal("fetch", fetchMock);

    await expect(pollNextTask("w")).resolves.toEqual(dispatch);
  });

  it("throws_with_error_message_on_non_2xx", async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      fakeResponse({ status: 500, body: { error: "DB exploded" } }),
    );
    vi.stubGlobal("fetch", fetchMock);

    await expect(pollNextTask("w")).rejects.toThrow("DB exploded");
  });

  it("throws_with_status_suffix_when_body_has_no_error_field", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValue(fakeResponse({ status: 502, body: {} }));
    vi.stubGlobal("fetch", fetchMock);

    await expect(pollNextTask("w")).rejects.toThrow("Poll failed with status 502");
  });

  it("throws_with_fallback_message_when_body_is_unparseable", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValue(fakeResponse({ status: 500, jsonThrows: true }));
    vi.stubGlobal("fetch", fetchMock);

    await expect(pollNextTask("w")).rejects.toThrow("Poll failed");
  });
});

// -------------------------------------------------------------------------
// submitTask
// -------------------------------------------------------------------------

describe("submitTask", () => {
  const req: SubmitTaskRequest = {
    worker_id: "w-1",
    stdout: "hi\n",
    stderr: "",
    duration_ms: 13.5,
  };

  it("posts_to_submit_endpoint_with_request_body_and_resolves_on_200", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValue(fakeResponse({ status: 200, body: {} }));
    vi.stubGlobal("fetch", fetchMock);

    await expect(submitTask("t-42", req)).resolves.toBeUndefined();

    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe("/api/tasks/t-42/submit");
    expect((init as RequestInit).method).toBe("POST");
    expect((init as RequestInit).headers).toMatchObject({
      "Content-Type": "application/json",
    });
    const body = JSON.parse((init as RequestInit).body as string);
    expect(body).toEqual(req);
  });

  it("throws_with_message_from_body_on_409", async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      fakeResponse({
        status: 409,
        body: { error: "Assignment is not accepting this submission" },
      }),
    );
    vi.stubGlobal("fetch", fetchMock);

    await expect(submitTask("t", req)).rejects.toThrow(
      "Assignment is not accepting this submission",
    );
  });

  it("throws_with_fallback_message_when_body_is_unparseable", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValue(fakeResponse({ status: 500, jsonThrows: true }));
    vi.stubGlobal("fetch", fetchMock);

    await expect(submitTask("t", req)).rejects.toThrow("Submit failed");
  });

  it("throws_with_status_suffix_when_body_has_no_error_field", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValue(fakeResponse({ status: 418, body: { detail: 1 } }));
    vi.stubGlobal("fetch", fetchMock);

    await expect(submitTask("t", req)).rejects.toThrow(
      "Submit failed with status 418",
    );
  });
});

// -------------------------------------------------------------------------
// getTaskStats
// -------------------------------------------------------------------------

describe("getTaskStats", () => {
  const stats = {
    pending: 5,
    in_flight: 2,
    completed_total: 10,
    completed_by_me: 3,
  };

  it("issues_get_with_query_params_and_returns_parsed_body_on_200", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValue(fakeResponse({ status: 200, body: stats }));
    vi.stubGlobal("fetch", fetchMock);

    const result = await getTaskStats("j", "w");
    expect(result).toEqual(stats);

    const [url] = fetchMock.mock.calls[0];
    expect(url).toMatch(/^\/api\/tasks\/stats\?/);
    const query = new URL(`http://x${url as string}`).searchParams;
    expect(query.get("job_id")).toBe("j");
    expect(query.get("worker_id")).toBe("w");
  });

  it("throws_with_message_from_body_on_404", async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      fakeResponse({ status: 404, body: { error: "Job not found" } }),
    );
    vi.stubGlobal("fetch", fetchMock);

    await expect(getTaskStats("missing", "w")).rejects.toThrow("Job not found");
  });

  it("throws_with_status_suffix_when_body_has_no_error_field", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValue(fakeResponse({ status: 500, body: {} }));
    vi.stubGlobal("fetch", fetchMock);

    await expect(getTaskStats("j", "w")).rejects.toThrow(
      "Stats fetch failed with status 500",
    );
  });

  it("throws_with_fallback_message_when_body_is_unparseable", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValue(fakeResponse({ status: 500, jsonThrows: true }));
    vi.stubGlobal("fetch", fetchMock);

    await expect(getTaskStats("j", "w")).rejects.toThrow("Stats fetch failed");
  });
});
