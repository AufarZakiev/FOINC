import { describe, it, expect, vi, beforeEach } from "vitest";
import { mount, flushPromises } from "@vue/test-utils";
import type { UploadCompleted } from "../../../../integrations/ui/events";
import type { StartJobResponse } from "../api";

// ---------------------------------------------------------------------------
// Mock `./api` before importing the component under test. StartJobButton
// imports `startJob` from "./api"; from this test file that's "../api".
// `vi.mock` is hoisted above imports, so we use `vi.hoisted` to lift the mock
// fn alongside it. Same pattern as `modules/upload/ui/__tests__/UploadForm.test.ts`.
// ---------------------------------------------------------------------------

const { mockStartJob } = vi.hoisted(() => ({
  mockStartJob:
    vi.fn<(jobId: string, chunkSize?: number) => Promise<StartJobResponse>>(),
}));

vi.mock("../api", () => ({
  startJob: mockStartJob,
}));

// Import the component AFTER the mock is set up.
import StartJobButton from "../StartJobButton.vue";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function makeUpload(overrides: Partial<UploadCompleted> = {}): UploadCompleted {
  return {
    jobId: "11111111-2222-3333-4444-555555555555",
    script: "print('hi')",
    csv: "a,b\n1,2\n",
    ...overrides,
  };
}

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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe("StartJobButton.vue — started event", () => {
  beforeEach(() => {
    mockStartJob.mockReset();
  });

  it("emits_started_with_JobStarted_payload_on_success", async () => {
    const upload = makeUpload();
    const response: StartJobResponse = {
      job_id: upload.jobId,
      task_count: 42,
    };
    mockStartJob.mockResolvedValue(response);

    const wrapper = mount(StartJobButton, { props: { upload } });
    await wrapper.find("button.btn--primary").trigger("click");
    await flushPromises();

    // startJob was called with the upload's jobId.
    expect(mockStartJob).toHaveBeenCalledTimes(1);
    expect(mockStartJob).toHaveBeenCalledWith(upload.jobId);

    // `started` emitted exactly once with the JobStarted payload.
    const emissions = wrapper.emitted("started");
    expect(emissions).toBeDefined();
    expect(emissions).toHaveLength(1);
    expect(emissions![0][0]).toEqual({
      jobId: upload.jobId,
      taskCount: 42,
    });
  });

  it("does_not_emit_notify_on_successful_start", async () => {
    mockStartJob.mockResolvedValue({
      job_id: "j",
      task_count: 1,
    });

    const wrapper = mount(StartJobButton, { props: { upload: makeUpload() } });
    await wrapper.find("button.btn--primary").trigger("click");
    await flushPromises();

    expect(wrapper.emitted("notify")).toBeUndefined();
  });
});

describe("StartJobButton.vue — notify event on failure", () => {
  beforeEach(() => {
    mockStartJob.mockReset();
  });

  it("emits_notify_with_error_level_on_rejection", async () => {
    mockStartJob.mockRejectedValue(new Error("server down"));

    const wrapper = mount(StartJobButton, { props: { upload: makeUpload() } });
    await wrapper.find("button.btn--primary").trigger("click");
    await flushPromises();

    const emissions = wrapper.emitted("notify");
    expect(emissions).toBeDefined();
    expect(emissions).toHaveLength(1);
    const payload = emissions![0][0] as { level: string; message: string };
    expect(payload.level).toBe("error");
    expect(payload.message).toBe("server down");
  });

  it("does_not_emit_started_on_rejection", async () => {
    mockStartJob.mockRejectedValue(new Error("boom"));

    const wrapper = mount(StartJobButton, { props: { upload: makeUpload() } });
    await wrapper.find("button.btn--primary").trigger("click");
    await flushPromises();

    expect(wrapper.emitted("started")).toBeUndefined();
  });

  it("emits_notify_with_fallback_message_when_error_is_not_an_Error_instance", async () => {
    // Something weird — a non-Error rejection value.
    mockStartJob.mockRejectedValue("plain string failure");

    const wrapper = mount(StartJobButton, { props: { upload: makeUpload() } });
    await wrapper.find("button.btn--primary").trigger("click");
    await flushPromises();

    const emissions = wrapper.emitted("notify");
    const payload = emissions![0][0] as { level: string; message: string };
    expect(payload.level).toBe("error");
    expect(payload.message).toBe("Failed to start job");
  });
});

describe("StartJobButton.vue — loading indicator + disabled state", () => {
  beforeEach(() => {
    mockStartJob.mockReset();
  });

  it("shows_spinner_and_disables_button_while_loading_then_resets_after_success", async () => {
    const deferred = makeDeferred<StartJobResponse>();
    mockStartJob.mockReturnValue(deferred.promise);

    const wrapper = mount(StartJobButton, { props: { upload: makeUpload() } });
    const button = wrapper.find("button.btn--primary")
      .element as HTMLButtonElement;

    // Idle before click: enabled, no spinner, label "Process all".
    expect(button.disabled).toBe(false);
    expect(wrapper.find(".spinner").exists()).toBe(false);
    expect(wrapper.text()).toContain("Process all");

    // Click: do NOT flush — we want to observe the mid-flight UI.
    await wrapper.find("button.btn--primary").trigger("click");
    await wrapper.vm.$nextTick();

    expect(button.disabled).toBe(true);
    expect(wrapper.find(".spinner").exists()).toBe(true);
    expect(wrapper.text()).toContain("Starting...");

    // Resolve and let everything flush — button re-enables.
    deferred.resolve({ job_id: "j", task_count: 1 });
    await flushPromises();

    expect(button.disabled).toBe(false);
    expect(wrapper.find(".spinner").exists()).toBe(false);
    expect(wrapper.text()).toContain("Process all");
  });

  it("re_enables_button_after_error", async () => {
    const deferred = makeDeferred<StartJobResponse>();
    mockStartJob.mockReturnValue(deferred.promise);

    const wrapper = mount(StartJobButton, { props: { upload: makeUpload() } });
    const button = wrapper.find("button.btn--primary")
      .element as HTMLButtonElement;

    await wrapper.find("button.btn--primary").trigger("click");
    await wrapper.vm.$nextTick();
    expect(button.disabled).toBe(true);

    deferred.reject(new Error("nope"));
    await flushPromises();

    expect(button.disabled).toBe(false);
    expect(wrapper.find(".spinner").exists()).toBe(false);
  });

  it("ignores_double_clicks_while_request_is_in_flight", async () => {
    const deferred = makeDeferred<StartJobResponse>();
    mockStartJob.mockReturnValue(deferred.promise);

    const wrapper = mount(StartJobButton, { props: { upload: makeUpload() } });

    // Two consecutive clicks while the mock is still pending.
    await wrapper.find("button.btn--primary").trigger("click");
    await wrapper.find("button.btn--primary").trigger("click");
    await wrapper.vm.$nextTick();

    // Only ONE startJob call — the loading flag prevents duplicate submits.
    expect(mockStartJob).toHaveBeenCalledTimes(1);

    deferred.resolve({ job_id: "j", task_count: 1 });
    await flushPromises();

    // And still exactly one emission.
    expect(wrapper.emitted("started")).toHaveLength(1);
  });
});
