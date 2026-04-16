import { describe, it, expect, vi, beforeEach } from "vitest";
import { mount, flushPromises } from "@vue/test-utils";
import type { DryRunResult } from "../dryRun";

// ---------------------------------------------------------------------------
// Mock `dryRun` before importing the Panel.
// Panel imports `dryRun` from "./dryRun"; from this test file that's "../dryRun".
// `vi.mock` is hoisted above imports, so we must use `vi.hoisted` to lift the
// mock fn alongside it.
// ---------------------------------------------------------------------------

const { mockDryRun } = vi.hoisted(() => ({
  mockDryRun:
    vi.fn<(script: string, csvRows: string[], header: string) => Promise<DryRunResult>>(),
}));

vi.mock("../dryRun", () => ({
  dryRun: mockDryRun,
}));

// Import panel + children after the mock is set up.
import DryRunPanel from "../DryRunPanel.vue";
import DryRunForm from "../DryRunForm.vue";
import DryRunResults from "../DryRunResults.vue";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/** Create a deferred promise for controlling async resolution in tests. */
function makeDeferred<T>(): {
  promise: Promise<T>;
  resolve: (v: T) => void;
  reject: (e: unknown) => void;
} {
  let resolve!: (v: T) => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

const samplePayload = {
  script: "print('hi')",
  header: "name,age",
  csvRows: ["Alice,30", "Bob,25"],
};

const sampleResult: DryRunResult = {
  rows: [
    { input: "Alice,30", stdout: "Alice,30", stderr: "", durationMs: 5 },
    { input: "Bob,25", stdout: "Bob,25", stderr: "", durationMs: 7 },
  ],
  totalDurationMs: 12,
};

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe("DryRunPanel.vue", () => {
  beforeEach(() => {
    mockDryRun.mockReset();
  });

  it("calls dryRun with (script, csvRows, header) when the form submits", async () => {
    mockDryRun.mockResolvedValue(sampleResult);

    const wrapper = mount(DryRunPanel);
    const form = wrapper.findComponent(DryRunForm);
    form.vm.$emit("submit", samplePayload);
    await flushPromises();

    expect(mockDryRun).toHaveBeenCalledTimes(1);
    expect(mockDryRun).toHaveBeenCalledWith(
      samplePayload.script,
      samplePayload.csvRows,
      samplePayload.header,
    );
  });

  it("shows a loading indicator while dryRun is pending", async () => {
    const deferred = makeDeferred<DryRunResult>();
    mockDryRun.mockReturnValue(deferred.promise);

    const wrapper = mount(DryRunPanel);
    const form = wrapper.findComponent(DryRunForm);
    form.vm.$emit("submit", samplePayload);
    await flushPromises();

    const loading = wrapper.find(".status--loading");
    expect(loading.exists()).toBe(true);
    expect(loading.text()).toContain("Running");

    // No results or error while loading.
    expect(wrapper.findComponent(DryRunResults).exists()).toBe(false);
    expect(wrapper.find(".status--error").exists()).toBe(false);

    // Resolve to clean up.
    deferred.resolve(sampleResult);
    await flushPromises();
  });

  it("renders DryRunResults with the result on success", async () => {
    mockDryRun.mockResolvedValue(sampleResult);

    const wrapper = mount(DryRunPanel);
    const form = wrapper.findComponent(DryRunForm);
    form.vm.$emit("submit", samplePayload);
    await flushPromises();

    const results = wrapper.findComponent(DryRunResults);
    expect(results.exists()).toBe(true);
    expect(results.props("result")).toEqual(sampleResult);

    // Loading/error should no longer be visible.
    expect(wrapper.find(".status--loading").exists()).toBe(false);
    expect(wrapper.find(".status--error").exists()).toBe(false);
  });

  it("shows an error message when dryRun rejects", async () => {
    mockDryRun.mockRejectedValue(new Error("Pyodide load failed"));

    const wrapper = mount(DryRunPanel);
    const form = wrapper.findComponent(DryRunForm);
    form.vm.$emit("submit", samplePayload);
    await flushPromises();

    const err = wrapper.find(".status--error");
    expect(err.exists()).toBe(true);
    expect(err.text()).toBe("Pyodide load failed");

    // Loading/results should not be visible.
    expect(wrapper.find(".status--loading").exists()).toBe(false);
    expect(wrapper.findComponent(DryRunResults).exists()).toBe(false);
  });

  it("uses a generic fallback message when the rejection is not an Error", async () => {
    mockDryRun.mockRejectedValue("something weird");

    const wrapper = mount(DryRunPanel);
    const form = wrapper.findComponent(DryRunForm);
    form.vm.$emit("submit", samplePayload);
    await flushPromises();

    const err = wrapper.find(".status--error");
    expect(err.exists()).toBe(true);
    expect(err.text()).toBe("Dry run failed");
  });

  it("clears the prior error on a new submit", async () => {
    mockDryRun.mockRejectedValueOnce(new Error("first failure"));

    const wrapper = mount(DryRunPanel);
    const form = wrapper.findComponent(DryRunForm);
    form.vm.$emit("submit", samplePayload);
    await flushPromises();

    expect(wrapper.find(".status--error").text()).toBe("first failure");

    // Second submit: still pending - prior error should already be cleared.
    const deferred = makeDeferred<DryRunResult>();
    mockDryRun.mockReturnValueOnce(deferred.promise);
    form.vm.$emit("submit", samplePayload);
    await flushPromises();

    // The prior error is gone; loading is shown.
    expect(wrapper.find(".status--error").exists()).toBe(false);
    expect(wrapper.find(".status--loading").exists()).toBe(true);

    // Clean up.
    deferred.resolve(sampleResult);
    await flushPromises();
  });

  it("clears the prior result on a new submit", async () => {
    mockDryRun.mockResolvedValueOnce(sampleResult);

    const wrapper = mount(DryRunPanel);
    const form = wrapper.findComponent(DryRunForm);
    form.vm.$emit("submit", samplePayload);
    await flushPromises();

    expect(wrapper.findComponent(DryRunResults).exists()).toBe(true);

    // Second submit: pending - prior result should already be cleared.
    const deferred = makeDeferred<DryRunResult>();
    mockDryRun.mockReturnValueOnce(deferred.promise);
    form.vm.$emit("submit", samplePayload);
    await flushPromises();

    // Prior results are gone; loading is shown.
    expect(wrapper.findComponent(DryRunResults).exists()).toBe(false);
    expect(wrapper.find(".status--loading").exists()).toBe(true);

    // Clean up.
    deferred.resolve(sampleResult);
    await flushPromises();
  });
});
