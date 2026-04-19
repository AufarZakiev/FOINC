import { describe, it, expect, vi, beforeEach } from "vitest";
import { mount, flushPromises } from "@vue/test-utils";
import type { Job } from "../../../../integrations/ui/types";

// ---------------------------------------------------------------------------
// Mock `./api` before importing the component under test.
// UploadForm imports `uploadFiles` from "./api"; from this test file that's
// "../api". `vi.mock` is hoisted above imports, so we use `vi.hoisted` to lift
// the mock fn alongside it.
// ---------------------------------------------------------------------------

const { mockUploadFiles } = vi.hoisted(() => ({
  mockUploadFiles: vi.fn<(csv: File, script: File) => Promise<Job>>(),
}));

vi.mock("../api", () => ({
  uploadFiles: mockUploadFiles,
}));

// Import component after the mock is set up.
import UploadForm from "../UploadForm.vue";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/**
 * Build a File stub whose `.text()` returns the given content. We avoid the
 * real File constructor's text reading (which depends on Blob internals in
 * jsdom) by overriding `text()` directly.
 */
function makeFile(name: string, content: string): File {
  const file = new File([content], name, { type: "text/plain" });
  // Override text() so assertions see exactly the content we passed in,
  // independent of jsdom's Blob.text() implementation.
  Object.defineProperty(file, "text", {
    value: () => Promise.resolve(content),
  });
  return file;
}

/**
 * Build a File stub whose `.text()` rejects with the given error. Used to
 * simulate a local file-read failure without network involvement.
 */
function makeFileWithFailingText(name: string, err: Error): File {
  const file = new File(["ignored"], name, { type: "text/plain" });
  Object.defineProperty(file, "text", {
    value: () => Promise.reject(err),
  });
  return file;
}

/**
 * Attach a file to an <input type="file"> and dispatch a change event so the
 * component's handler runs. jsdom does not allow assigning to `input.files`
 * directly, so we redefine the property.
 */
async function setInputFile(
  input: HTMLInputElement,
  file: File,
): Promise<void> {
  Object.defineProperty(input, "files", {
    value: [file],
    configurable: true,
  });
  input.dispatchEvent(new Event("change"));
  await flushPromises();
}

const sampleJob: Job = {
  job_id: "11111111-2222-3333-4444-555555555555",
  csv_filename: "data.csv",
  script_filename: "script.py",
  csv_size_bytes: 12,
  script_size_bytes: 18,
  status: "uploaded",
  created_at: "2026-01-01T00:00:00Z",
};

// ---------------------------------------------------------------------------
// Tests: uploaded event (success path)
// ---------------------------------------------------------------------------

describe("UploadForm.vue — uploaded event", () => {
  beforeEach(() => {
    mockUploadFiles.mockReset();
  });

  it("emits_uploaded_once_on_successful_upload", async () => {
    mockUploadFiles.mockResolvedValue(sampleJob);

    const wrapper = mount(UploadForm);
    const inputs = wrapper.findAll('input[type="file"]');
    const csvInput = inputs[0].element as HTMLInputElement;
    const scriptInput = inputs[1].element as HTMLInputElement;

    await setInputFile(csvInput, makeFile("data.csv", "a,b\n1,2\n"));
    await setInputFile(scriptInput, makeFile("script.py", "print('hi')\n"));

    await wrapper.find("button.btn--primary").trigger("click");
    await flushPromises();

    const emissions = wrapper.emitted("uploaded");
    expect(emissions).toBeDefined();
    expect(emissions).toHaveLength(1);
  });

  it("emits_uploaded_with_jobId_from_backend_response", async () => {
    mockUploadFiles.mockResolvedValue(sampleJob);

    const wrapper = mount(UploadForm);
    const inputs = wrapper.findAll('input[type="file"]');
    await setInputFile(
      inputs[0].element as HTMLInputElement,
      makeFile("data.csv", "a,b\n1,2\n"),
    );
    await setInputFile(
      inputs[1].element as HTMLInputElement,
      makeFile("script.py", "print('hi')\n"),
    );

    await wrapper.find("button.btn--primary").trigger("click");
    await flushPromises();

    const payload = wrapper.emitted("uploaded")![0][0] as {
      jobId: string;
      script: string;
      csv: string;
    };
    expect(payload.jobId).toBe(sampleJob.job_id);
  });

  it("emits_uploaded_with_script_text_from_input_file", async () => {
    mockUploadFiles.mockResolvedValue(sampleJob);

    const scriptSource = "import sys\nprint(sys.argv)\n";

    const wrapper = mount(UploadForm);
    const inputs = wrapper.findAll('input[type="file"]');
    await setInputFile(
      inputs[0].element as HTMLInputElement,
      makeFile("data.csv", "a,b\n1,2\n"),
    );
    await setInputFile(
      inputs[1].element as HTMLInputElement,
      makeFile("script.py", scriptSource),
    );

    await wrapper.find("button.btn--primary").trigger("click");
    await flushPromises();

    const payload = wrapper.emitted("uploaded")![0][0] as {
      jobId: string;
      script: string;
      csv: string;
    };
    expect(payload.script).toBe(scriptSource);
  });

  it("emits_uploaded_with_csv_text_from_input_file", async () => {
    mockUploadFiles.mockResolvedValue(sampleJob);

    const csvSource = "name,age\nAlice,30\nBob,25\n";

    const wrapper = mount(UploadForm);
    const inputs = wrapper.findAll('input[type="file"]');
    await setInputFile(
      inputs[0].element as HTMLInputElement,
      makeFile("data.csv", csvSource),
    );
    await setInputFile(
      inputs[1].element as HTMLInputElement,
      makeFile("script.py", "print('hi')\n"),
    );

    await wrapper.find("button.btn--primary").trigger("click");
    await flushPromises();

    const payload = wrapper.emitted("uploaded")![0][0] as {
      jobId: string;
      script: string;
      csv: string;
    };
    expect(payload.csv).toBe(csvSource);
  });

  it("emits_uploaded_payload_matches_exact_file_contents", async () => {
    mockUploadFiles.mockResolvedValue(sampleJob);

    // Distinct contents in each file to guarantee no swap/reuse.
    const scriptSource = "# SCRIPT_MARKER_9f2a\nprint('from script')\n";
    const csvSource = "HEADER_MARKER_4b1c\nrow1,row2\n";

    const wrapper = mount(UploadForm);
    const inputs = wrapper.findAll('input[type="file"]');
    await setInputFile(
      inputs[0].element as HTMLInputElement,
      makeFile("data.csv", csvSource),
    );
    await setInputFile(
      inputs[1].element as HTMLInputElement,
      makeFile("script.py", scriptSource),
    );

    await wrapper.find("button.btn--primary").trigger("click");
    await flushPromises();

    const payload = wrapper.emitted("uploaded")![0][0] as {
      jobId: string;
      script: string;
      csv: string;
    };
    expect(payload).toEqual({
      jobId: sampleJob.job_id,
      script: scriptSource,
      csv: csvSource,
    });
  });

  it("does_not_emit_notify_on_successful_upload", async () => {
    mockUploadFiles.mockResolvedValue(sampleJob);

    const wrapper = mount(UploadForm);
    const inputs = wrapper.findAll('input[type="file"]');
    await setInputFile(
      inputs[0].element as HTMLInputElement,
      makeFile("data.csv", "a,b\n1,2\n"),
    );
    await setInputFile(
      inputs[1].element as HTMLInputElement,
      makeFile("script.py", "print('hi')\n"),
    );

    await wrapper.find("button.btn--primary").trigger("click");
    await flushPromises();

    expect(wrapper.emitted("notify")).toBeUndefined();
  });
});

// ---------------------------------------------------------------------------
// Tests: notify event + failure paths
// ---------------------------------------------------------------------------

describe("UploadForm.vue — notify event on failure", () => {
  beforeEach(() => {
    mockUploadFiles.mockReset();
  });

  it("emits_notify_with_error_level_and_message_when_upload_rejects", async () => {
    mockUploadFiles.mockRejectedValue(new Error("network down"));

    const wrapper = mount(UploadForm);
    const inputs = wrapper.findAll('input[type="file"]');
    await setInputFile(
      inputs[0].element as HTMLInputElement,
      makeFile("data.csv", "a,b\n1,2\n"),
    );
    await setInputFile(
      inputs[1].element as HTMLInputElement,
      makeFile("script.py", "print('hi')\n"),
    );

    await wrapper.find("button.btn--primary").trigger("click");
    await flushPromises();

    const emissions = wrapper.emitted("notify");
    expect(emissions).toBeDefined();
    expect(emissions).toHaveLength(1);
    const payload = emissions![0][0] as { level: string; message: string };
    expect(payload.level).toBe("error");
    expect(payload.message).toBe("network down");
  });

  it("does_not_emit_uploaded_when_backend_upload_rejects", async () => {
    mockUploadFiles.mockRejectedValue(new Error("network down"));

    const wrapper = mount(UploadForm);
    const inputs = wrapper.findAll('input[type="file"]');
    await setInputFile(
      inputs[0].element as HTMLInputElement,
      makeFile("data.csv", "a,b\n1,2\n"),
    );
    await setInputFile(
      inputs[1].element as HTMLInputElement,
      makeFile("script.py", "print('hi')\n"),
    );

    await wrapper.find("button.btn--primary").trigger("click");
    await flushPromises();

    expect(wrapper.emitted("uploaded")).toBeUndefined();
  });

  it("emits_notify_with_error_when_file_read_fails", async () => {
    mockUploadFiles.mockResolvedValue(sampleJob);

    const wrapper = mount(UploadForm);
    const inputs = wrapper.findAll('input[type="file"]');
    await setInputFile(
      inputs[0].element as HTMLInputElement,
      makeFile("data.csv", "a,b\n1,2\n"),
    );
    // Script file whose .text() rejects — simulates a local read failure.
    await setInputFile(
      inputs[1].element as HTMLInputElement,
      makeFileWithFailingText("script.py", new Error("read failed")),
    );

    await wrapper.find("button.btn--primary").trigger("click");
    await flushPromises();

    const emissions = wrapper.emitted("notify");
    expect(emissions).toBeDefined();
    expect(emissions).toHaveLength(1);
    const payload = emissions![0][0] as { level: string; message: string };
    expect(payload.level).toBe("error");
    expect(payload.message).toBe("read failed");
  });

  it("does_not_emit_uploaded_when_file_read_fails", async () => {
    mockUploadFiles.mockResolvedValue(sampleJob);

    const wrapper = mount(UploadForm);
    const inputs = wrapper.findAll('input[type="file"]');
    await setInputFile(
      inputs[0].element as HTMLInputElement,
      makeFile("data.csv", "a,b\n1,2\n"),
    );
    await setInputFile(
      inputs[1].element as HTMLInputElement,
      makeFileWithFailingText("script.py", new Error("read failed")),
    );

    await wrapper.find("button.btn--primary").trigger("click");
    await flushPromises();

    expect(wrapper.emitted("uploaded")).toBeUndefined();
  });

  it("does_not_emit_uploaded_when_script_has_wrong_extension", async () => {
    mockUploadFiles.mockResolvedValue(sampleJob);

    const wrapper = mount(UploadForm);
    const inputs = wrapper.findAll('input[type="file"]');
    await setInputFile(
      inputs[0].element as HTMLInputElement,
      makeFile("data.csv", "a,b\n1,2\n"),
    );
    // .txt where .py is required — validation should reject this.
    await setInputFile(
      inputs[1].element as HTMLInputElement,
      makeFile("script.txt", "print('hi')\n"),
    );

    // The upload button should be disabled because script is null after
    // validation, so clicking does nothing.
    const button = wrapper.find("button.btn--primary")
      .element as HTMLButtonElement;
    expect(button.disabled).toBe(true);

    await wrapper.find("button.btn--primary").trigger("click");
    await flushPromises();

    expect(wrapper.emitted("uploaded")).toBeUndefined();
    expect(mockUploadFiles).not.toHaveBeenCalled();
  });

  it("does_not_emit_uploaded_when_csv_has_wrong_extension", async () => {
    mockUploadFiles.mockResolvedValue(sampleJob);

    const wrapper = mount(UploadForm);
    const inputs = wrapper.findAll('input[type="file"]');
    // .txt where .csv is required — validation should reject this.
    await setInputFile(
      inputs[0].element as HTMLInputElement,
      makeFile("data.txt", "a,b\n1,2\n"),
    );
    await setInputFile(
      inputs[1].element as HTMLInputElement,
      makeFile("script.py", "print('hi')\n"),
    );

    const button = wrapper.find("button.btn--primary")
      .element as HTMLButtonElement;
    expect(button.disabled).toBe(true);

    await wrapper.find("button.btn--primary").trigger("click");
    await flushPromises();

    expect(wrapper.emitted("uploaded")).toBeUndefined();
    expect(mockUploadFiles).not.toHaveBeenCalled();
  });
});

// ---------------------------------------------------------------------------
// Tests: in-flight loading indicator + disabled button
// ---------------------------------------------------------------------------

describe("UploadForm.vue — loading indicator", () => {
  beforeEach(() => {
    mockUploadFiles.mockReset();
  });

  it("renders_loading_indicator_while_upload_is_in_flight_and_hides_after_resolution", async () => {
    // Deferred promise — resolve only after we've asserted the loading state.
    let resolve!: (j: Job) => void;
    const pending = new Promise<Job>((res) => {
      resolve = res;
    });
    mockUploadFiles.mockReturnValue(pending);

    const wrapper = mount(UploadForm);
    const inputs = wrapper.findAll('input[type="file"]');
    await setInputFile(
      inputs[0].element as HTMLInputElement,
      makeFile("data.csv", "a,b\n1,2\n"),
    );
    await setInputFile(
      inputs[1].element as HTMLInputElement,
      makeFile("script.py", "print('hi')\n"),
    );

    // Before click: no loading indicator.
    expect(wrapper.find(".loading-indicator").exists()).toBe(false);

    // Click — upload promise is pending (not yet resolved).
    await wrapper.find("button.btn--primary").trigger("click");
    // No flushPromises yet — we want to observe the mid-flight UI.
    await wrapper.vm.$nextTick();

    // Indicator should be visible while in flight.
    const indicator = wrapper.find(".loading-indicator");
    expect(indicator.exists()).toBe(true);
    expect(indicator.text()).toContain("Uploading...");

    // Resolve the pending upload and let everything flush.
    resolve(sampleJob);
    await flushPromises();

    // Indicator should be gone after resolution.
    expect(wrapper.find(".loading-indicator").exists()).toBe(false);
  });

  it("disables_upload_button_while_upload_is_in_flight", async () => {
    let resolve!: (j: Job) => void;
    const pending = new Promise<Job>((res) => {
      resolve = res;
    });
    mockUploadFiles.mockReturnValue(pending);

    const wrapper = mount(UploadForm);
    const inputs = wrapper.findAll('input[type="file"]');
    await setInputFile(
      inputs[0].element as HTMLInputElement,
      makeFile("data.csv", "a,b\n1,2\n"),
    );
    await setInputFile(
      inputs[1].element as HTMLInputElement,
      makeFile("script.py", "print('hi')\n"),
    );

    const button = wrapper.find("button.btn--primary")
      .element as HTMLButtonElement;

    // Before click with both files present: enabled.
    expect(button.disabled).toBe(false);

    await wrapper.find("button.btn--primary").trigger("click");
    await wrapper.vm.$nextTick();

    // During in-flight request: disabled.
    expect(button.disabled).toBe(true);

    resolve(sampleJob);
    await flushPromises();

    // After resolution: button re-enabled (files still set).
    expect(button.disabled).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// Tests: sanity — no UploadStatus component anywhere
// ---------------------------------------------------------------------------

describe("UploadForm.vue — removed UploadStatus component", () => {
  beforeEach(() => {
    mockUploadFiles.mockReset();
  });

  it("does_not_render_any_UploadStatus_child_component", () => {
    const wrapper = mount(UploadForm);
    // Look up by name; if the component is not registered / not present, this
    // returns an empty wrapper and .exists() is false.
    const maybe = wrapper.findComponent({ name: "UploadStatus" });
    expect(maybe.exists()).toBe(false);
  });
});
