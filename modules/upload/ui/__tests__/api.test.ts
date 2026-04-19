import { describe, it, expect, vi, afterEach } from "vitest";
import { deleteJob } from "../api";

/**
 * Build a minimal Response-like object that satisfies the subset of the fetch
 * API surface that `deleteJob` uses: `status`, `ok`, and `json()`. We avoid
 * the real `Response` constructor so we can drive arbitrary status codes
 * (including 204 with no body) without fighting the DOM `Response` semantics
 * in jsdom.
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

describe("deleteJob", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  it("resolves_on_204_no_content", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValue(fakeResponse({ status: 204, jsonThrows: true }));
    vi.stubGlobal("fetch", fetchMock);

    await expect(deleteJob("abc-123")).resolves.toBeUndefined();

    expect(fetchMock).toHaveBeenCalledTimes(1);
    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe("/api/jobs/abc-123");
    expect((init as RequestInit).method).toBe("DELETE");
  });

  it("throws_with_message_from_body_on_404", async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      fakeResponse({ status: 404, body: { error: "Job not found" } }),
    );
    vi.stubGlobal("fetch", fetchMock);

    await expect(deleteJob("missing")).rejects.toThrow("Job not found");
  });

  it("throws_with_fallback_message_on_500_with_no_body", async () => {
    // When the server returns no JSON body, `response.json()` rejects and the
    // api layer falls back to a generic "Delete failed" message (not the
    // status-suffixed one, which only fires when the body parsed but had no
    // `error` field).
    const fetchMock = vi
      .fn()
      .mockResolvedValue(fakeResponse({ status: 500, jsonThrows: true }));
    vi.stubGlobal("fetch", fetchMock);

    await expect(deleteJob("boom")).rejects.toThrow("Delete failed");
  });

  it("throws_with_status_suffix_when_body_is_parseable_but_has_no_error_field", async () => {
    // Body parses as JSON but has no `error` key — status-suffixed fallback
    // kicks in via `body.error ?? \`Delete failed with status ${status}\``.
    const fetchMock = vi
      .fn()
      .mockResolvedValue(fakeResponse({ status: 500, body: { detail: "x" } }));
    vi.stubGlobal("fetch", fetchMock);

    await expect(deleteJob("boom")).rejects.toThrow(
      "Delete failed with status 500",
    );
  });

  it("throws_on_non_204_status_even_when_body_has_no_error_field", async () => {
    // Some 4xx / 5xx might come back with an unexpected JSON shape (no
    // `error` key). We should still throw with the status-fallback message
    // rather than silently resolving.
    const fetchMock = vi
      .fn()
      .mockResolvedValue(fakeResponse({ status: 418, body: { other: 1 } }));
    vi.stubGlobal("fetch", fetchMock);

    await expect(deleteJob("teapot")).rejects.toThrow(
      "Delete failed with status 418",
    );
  });

  it("throws_on_200_ok_non_204_status", async () => {
    // Spec: resolves ONLY on 204. Any other non-204 status must throw —
    // including 200, which for a DELETE endpoint is unexpected.
    const fetchMock = vi
      .fn()
      .mockResolvedValue(fakeResponse({ status: 200, body: { ok: true } }));
    vi.stubGlobal("fetch", fetchMock);

    await expect(deleteJob("weird")).rejects.toThrow(
      "Delete failed with status 200",
    );
  });
});
