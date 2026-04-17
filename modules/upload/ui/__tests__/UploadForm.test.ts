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
// Tests
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

  it("shows_error_message_when_backend_upload_rejects", async () => {
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

    expect(wrapper.text()).toContain("network down");
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
});
