import { describe, it, expect, vi, beforeEach } from "vitest";
import { mount, flushPromises } from "@vue/test-utils";
import type { DryRunResult } from "../dryRun";
import type { UploadCompleted } from "../../../../integrations/ui/events";

// ---------------------------------------------------------------------------
// Mock `dryRun` before importing the Panel.
// Panel imports `dryRun` from "./dryRun"; from this test file that's "../dryRun".
// `vi.mock` is hoisted above imports, so we must use `vi.hoisted` to lift the
// mock fn alongside it.
// ---------------------------------------------------------------------------

const { mockDryRun } = vi.hoisted(() => ({
  mockDryRun:
    vi.fn<(script: string, csvRows: string[]) => Promise<DryRunResult>>(),
}));

vi.mock("../dryRun", () => ({
  dryRun: mockDryRun,
}));

// Import panel + children after the mock is set up.
import DryRunPanel from "../DryRunPanel.vue";
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

function makeUpload(overrides: Partial<UploadCompleted> = {}): UploadCompleted {
  return {
    jobId: "job-1",
    script: "print('hi')",
    csv: "name,age\nAlice,30\nBob,25",
    ...overrides,
  };
}

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

  // ---- Run button rendering ----------------------------------------------

  it("renders the Run button when upload has 1 valid data row", () => {
    const upload = makeUpload({ csv: "name,age\nAlice,30" });
    const wrapper = mount(DryRunPanel, { props: { upload } });

    const btn = wrapper.find("button.btn--primary");
    expect(btn.exists()).toBe(true);
    expect(btn.text()).toBe("Run dry run");
  });

  it("renders the Run button when upload has 2 valid data rows", () => {
    const upload = makeUpload({ csv: "name,age\nAlice,30\nBob,25" });
    const wrapper = mount(DryRunPanel, { props: { upload } });

    const btn = wrapper.find("button.btn--primary");
    expect(btn.exists()).toBe(true);
    expect(btn.text()).toBe("Run dry run");
  });

  it("renders the Run button when upload has 3 valid data rows", () => {
    const upload = makeUpload({ csv: "name,age\nAlice,30\nBob,25\nCarol,40" });
    const wrapper = mount(DryRunPanel, { props: { upload } });

    const btn = wrapper.find("button.btn--primary");
    expect(btn.exists()).toBe(true);
    expect(btn.text()).toBe("Run dry run");
  });

  // ---- dryRun is called with (script, csvRows) - two args, no header -----

  it("calls dryRun(upload.script, csvRows) on button click - two args, no header", async () => {
    mockDryRun.mockResolvedValue(sampleResult);

    const upload = makeUpload({
      script: "print('hi')",
      csv: "name,age\nAlice,30\nBob,25",
    });
    const wrapper = mount(DryRunPanel, { props: { upload } });

    await wrapper.find("button.btn--primary").trigger("click");
    await flushPromises();

    expect(mockDryRun).toHaveBeenCalledTimes(1);
    expect(mockDryRun).toHaveBeenCalledWith("print('hi')", [
      "Alice,30",
      "Bob,25",
    ]);
    // Ensure exactly 2 args (no header arg).
    expect(mockDryRun.mock.calls[0]).toHaveLength(2);
  });

  // ---- Loading state ------------------------------------------------------

  it("shows a loading indicator and hides the Run button while dryRun is pending", async () => {
    const deferred = makeDeferred<DryRunResult>();
    mockDryRun.mockReturnValue(deferred.promise);

    const upload = makeUpload();
    const wrapper = mount(DryRunPanel, { props: { upload } });

    await wrapper.find("button.btn--primary").trigger("click");
    await flushPromises();

    const loading = wrapper.find(".status--loading");
    expect(loading.exists()).toBe(true);
    expect(loading.text()).toContain("Running");

    // Run button is hidden while loading.
    expect(wrapper.find("button.btn--primary").exists()).toBe(false);

    // No results or error while loading.
    expect(wrapper.findComponent(DryRunResults).exists()).toBe(false);
    expect(wrapper.find(".status--error").exists()).toBe(false);

    // Resolve to clean up.
    deferred.resolve(sampleResult);
    await flushPromises();
  });

  // ---- Success state ------------------------------------------------------

  it("renders DryRunResults with the returned result on success", async () => {
    mockDryRun.mockResolvedValue(sampleResult);

    const upload = makeUpload();
    const wrapper = mount(DryRunPanel, { props: { upload } });

    await wrapper.find("button.btn--primary").trigger("click");
    await flushPromises();

    const results = wrapper.findComponent(DryRunResults);
    expect(results.exists()).toBe(true);
    expect(results.props("result")).toEqual(sampleResult);

    // Loading / error / Run button should not be visible.
    expect(wrapper.find(".status--loading").exists()).toBe(false);
    expect(wrapper.find(".status--error").exists()).toBe(false);
    expect(wrapper.find("button.btn--primary").exists()).toBe(false);
  });

  // ---- Error state --------------------------------------------------------

  it("shows an error message from Error.message when dryRun rejects", async () => {
    mockDryRun.mockRejectedValue(new Error("Pyodide load failed"));

    const upload = makeUpload();
    const wrapper = mount(DryRunPanel, { props: { upload } });

    await wrapper.find("button.btn--primary").trigger("click");
    await flushPromises();

    const err = wrapper.find(".status--error");
    expect(err.exists()).toBe(true);
    expect(err.text()).toBe("Pyodide load failed");

    // Loading / results should not be visible.
    expect(wrapper.find(".status--loading").exists()).toBe(false);
    expect(wrapper.findComponent(DryRunResults).exists()).toBe(false);
  });

  // ---- CSV parsing --------------------------------------------------------

  it("parses upload.csv correctly: drops header, takes 'h,a,b\\n1,2\\n3,4' → ['1,2','3,4']", async () => {
    mockDryRun.mockResolvedValue(sampleResult);

    const upload = makeUpload({ csv: "h,a,b\n1,2\n3,4" });
    const wrapper = mount(DryRunPanel, { props: { upload } });

    await wrapper.find("button.btn--primary").trigger("click");
    await flushPromises();

    expect(mockDryRun).toHaveBeenCalledWith(upload.script, ["1,2", "3,4"]);
  });

  it("slices CSV with more than 3 data rows to the first 3", async () => {
    mockDryRun.mockResolvedValue(sampleResult);

    const upload = makeUpload({
      csv: "h1,h2\nr1a,r1b\nr2a,r2b\nr3a,r3b\nr4a,r4b\nr5a,r5b",
    });
    const wrapper = mount(DryRunPanel, { props: { upload } });

    await wrapper.find("button.btn--primary").trigger("click");
    await flushPromises();

    expect(mockDryRun).toHaveBeenCalledWith(upload.script, [
      "r1a,r1b",
      "r2a,r2b",
      "r3a,r3b",
    ]);
  });

  it("discards whitespace-only and empty lines before counting rows", async () => {
    mockDryRun.mockResolvedValue(sampleResult);

    const upload = makeUpload({
      // Blank first line, whitespace-only lines, trailing/leading whitespace.
      // First non-empty line is the header and should be dropped.
      csv: "\n   \n  name,age  \n Alice,30 \n\n Bob,25 \n   \n",
    });
    const wrapper = mount(DryRunPanel, { props: { upload } });

    await wrapper.find("button.btn--primary").trigger("click");
    await flushPromises();

    expect(mockDryRun).toHaveBeenCalledWith(upload.script, [
      "Alice,30",
      "Bob,25",
    ]);
  });

  it("discards whitespace-only lines when counting >3 data rows (still slices to 3)", async () => {
    mockDryRun.mockResolvedValue(sampleResult);

    const upload = makeUpload({
      csv: "h,a\n\n   \nr1\n\nr2\n   \nr3\n\nr4",
    });
    const wrapper = mount(DryRunPanel, { props: { upload } });

    await wrapper.find("button.btn--primary").trigger("click");
    await flushPromises();

    expect(mockDryRun).toHaveBeenCalledWith(upload.script, ["r1", "r2", "r3"]);
  });

  // ---- Validation: 0 data rows -------------------------------------------

  it("renders inline error and no Run button when CSV has only a header", () => {
    const upload = makeUpload({ csv: "name,age" });
    const wrapper = mount(DryRunPanel, { props: { upload } });

    const err = wrapper.find(".status--error");
    expect(err.exists()).toBe(true);
    expect(err.text()).toBe("CSV must contain at least 1 data row");

    // Run button must not exist in this state.
    expect(wrapper.find("button.btn--primary").exists()).toBe(false);
  });

  it("renders inline error and no Run button when CSV is empty", () => {
    const upload = makeUpload({ csv: "" });
    const wrapper = mount(DryRunPanel, { props: { upload } });

    const err = wrapper.find(".status--error");
    expect(err.exists()).toBe(true);
    expect(err.text()).toBe("CSV must contain at least 1 data row");

    expect(wrapper.find("button.btn--primary").exists()).toBe(false);
  });

  it("renders inline error and no Run button when CSV is only whitespace", () => {
    const upload = makeUpload({ csv: "   \n\n   \n" });
    const wrapper = mount(DryRunPanel, { props: { upload } });

    const err = wrapper.find(".status--error");
    expect(err.exists()).toBe(true);
    expect(err.text()).toBe("CSV must contain at least 1 data row");

    expect(wrapper.find("button.btn--primary").exists()).toBe(false);
  });

  it("does not call dryRun when the CSV yields 0 data rows (no button to click)", async () => {
    const upload = makeUpload({ csv: "name,age" });
    const wrapper = mount(DryRunPanel, { props: { upload } });

    // Confirm no button exists - clicking is not possible.
    expect(wrapper.find("button.btn--primary").exists()).toBe(false);

    // Give the component a chance to do something (it shouldn't).
    await flushPromises();

    expect(mockDryRun).not.toHaveBeenCalled();
  });

  // ---- Reactivity: upload prop change ------------------------------------

  it("resets error to null when the upload prop changes", async () => {
    mockDryRun.mockRejectedValueOnce(new Error("first failure"));

    const upload1 = makeUpload({
      jobId: "job-1",
      script: "print(1)",
      csv: "h\nr1",
    });
    const wrapper = mount(DryRunPanel, { props: { upload: upload1 } });

    await wrapper.find("button.btn--primary").trigger("click");
    await flushPromises();

    // Sanity: error is shown.
    const err = wrapper.find(".status--error");
    expect(err.exists()).toBe(true);
    expect(err.text()).toBe("first failure");

    // Change the prop to a new payload.
    const upload2 = makeUpload({
      jobId: "job-2",
      script: "print(2)",
      csv: "h\nr2",
    });
    await wrapper.setProps({ upload: upload2 });
    await flushPromises();

    // The prior error is gone; we're back to the idle Run-button state.
    expect(wrapper.find(".status--error").exists()).toBe(false);
    expect(wrapper.find("button.btn--primary").exists()).toBe(true);
  });

  it("resets result to null when the upload prop changes", async () => {
    mockDryRun.mockResolvedValueOnce(sampleResult);

    const upload1 = makeUpload({
      jobId: "job-1",
      script: "print(1)",
      csv: "h\nr1",
    });
    const wrapper = mount(DryRunPanel, { props: { upload: upload1 } });

    await wrapper.find("button.btn--primary").trigger("click");
    await flushPromises();

    // Sanity: result rendered.
    expect(wrapper.findComponent(DryRunResults).exists()).toBe(true);

    // Change the prop to a new payload.
    const upload2 = makeUpload({
      jobId: "job-2",
      script: "print(2)",
      csv: "h\nr2",
    });
    await wrapper.setProps({ upload: upload2 });
    await flushPromises();

    // Prior result is gone; we're back to the idle Run-button state.
    expect(wrapper.findComponent(DryRunResults).exists()).toBe(false);
    expect(wrapper.find("button.btn--primary").exists()).toBe(true);
  });
});
