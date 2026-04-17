import { describe, it, expect, beforeEach } from "vitest";
import { mount, flushPromises, type VueWrapper } from "@vue/test-utils";
import DryRunForm from "../DryRunForm.vue";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/**
 * Build a File-like object that DryRunForm can accept via a file input.
 * Uses the real File constructor (available in jsdom).
 */
function makeFile(name: string, contents: string, type = "text/plain"): File {
  return new File([contents], name, { type });
}

/**
 * Set files on an input[type=file] element. jsdom supports defining the
 * `files` property via Object.defineProperty.
 */
function setFiles(input: HTMLInputElement, files: File[]): void {
  Object.defineProperty(input, "files", {
    configurable: true,
    value: files,
  });
}

async function setScriptFile(wrapper: VueWrapper, file: File) {
  const input = wrapper.find<HTMLInputElement>('input[type="file"][accept=".py"]');
  setFiles(input.element, [file]);
  await input.trigger("change");
}

async function setCsvFile(wrapper: VueWrapper, file: File) {
  const input = wrapper.find<HTMLInputElement>('input[type="file"][accept=".csv"]');
  setFiles(input.element, [file]);
  await input.trigger("change");
}

async function switchScriptToInline(wrapper: VueWrapper) {
  // The first toggle group belongs to the script field.
  const buttons = wrapper.findAll(".field").at(0)!.findAll("button.toggle__btn");
  await buttons[1].trigger("click"); // "Inline"
}

async function switchCsvToInline(wrapper: VueWrapper) {
  const buttons = wrapper.findAll(".field").at(1)!.findAll("button.toggle__btn");
  await buttons[1].trigger("click"); // "Inline"
}

/**
 * Wait for any pending FileReader reads (which settle on a macrotask in jsdom)
 * plus any Vue microtasks to flush.
 */
async function drainMacrotasks(): Promise<void> {
  for (let i = 0; i < 10; i++) {
    await new Promise((r) => setTimeout(r, 0));
    await flushPromises();
  }
}

async function clickSubmit(wrapper: VueWrapper) {
  const btn = wrapper.find("button.btn--primary");
  await btn.trigger("click");
  // handleSubmit awaits up to two FileReader reads; jsdom's FileReader fires
  // onload on a macrotask (setTimeout-style), so flushPromises alone is not
  // enough. Drain macrotasks several times so the emit completes.
  await drainMacrotasks();
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe("DryRunForm.vue", () => {
  let wrapper: VueWrapper;

  beforeEach(() => {
    wrapper = mount(DryRunForm);
  });

  // ---- Happy path: file+file ------------------------------------------

  it("emits submit with correct payload when valid .py and .csv files are provided", async () => {
    const scriptFile = makeFile("script.py", "print('hi')");
    const csvFile = makeFile(
      "data.csv",
      "name,age\nAlice,30\nBob,25",
      "text/csv",
    );

    await setScriptFile(wrapper, scriptFile);
    await setCsvFile(wrapper, csvFile);
    await clickSubmit(wrapper);

    const emissions = wrapper.emitted("submit");
    expect(emissions).toBeTruthy();
    expect(emissions).toHaveLength(1);

    const payload = emissions![0][0] as {
      script: string;
      csvRows: string[];
    };
    expect(payload.script).toBe("print('hi')");
    expect(payload).not.toHaveProperty("header");
    expect(payload.csvRows).toEqual(["Alice,30", "Bob,25"]);
  });

  // ---- Wrong extension -----------------------------------------------

  it("does not emit and shows an inline error when script file has wrong extension", async () => {
    const badScript = makeFile("script.txt", "print('hi')");
    await setScriptFile(wrapper, badScript);

    // The error is shown inline.
    const errorText = wrapper.find(".field__error").text();
    expect(errorText).toContain("Expected a .py file");

    // Submit should be disabled because no valid script was accepted.
    const btn = wrapper.find<HTMLButtonElement>("button.btn--primary");
    expect(btn.element.disabled).toBe(true);
    expect(wrapper.emitted("submit")).toBeFalsy();
  });

  it("does not emit and shows an inline error when CSV file has wrong extension", async () => {
    const scriptFile = makeFile("script.py", "print('hi')");
    const badCsv = makeFile("data.txt", "name,age\nAlice,30");
    await setScriptFile(wrapper, scriptFile);
    await setCsvFile(wrapper, badCsv);

    const errors = wrapper.findAll(".field__error");
    const hasCsvError = errors.some((e) =>
      e.text().includes("Expected a .csv file"),
    );
    expect(hasCsvError).toBe(true);

    const btn = wrapper.find<HTMLButtonElement>("button.btn--primary");
    expect(btn.element.disabled).toBe(true);
    expect(wrapper.emitted("submit")).toBeFalsy();
  });

  // ---- Inline textarea inputs ----------------------------------------

  it("emits submit with correct payload when inline textarea inputs are used", async () => {
    await switchScriptToInline(wrapper);
    await switchCsvToInline(wrapper);

    const textareas = wrapper.findAll("textarea");
    expect(textareas).toHaveLength(2);
    await textareas[0].setValue("print('hello')");
    await textareas[1].setValue("a,b\n1,2\n3,4");

    await clickSubmit(wrapper);

    const emissions = wrapper.emitted("submit");
    expect(emissions).toHaveLength(1);
    const payload = emissions![0][0] as {
      script: string;
      csvRows: string[];
    };
    expect(payload.script).toBe("print('hello')");
    expect(payload).not.toHaveProperty("header");
    expect(payload.csvRows).toEqual(["1,2", "3,4"]);
  });

  // ---- CSV parsing: trimming, empty-line handling, header is dropped ----

  it("trims lines, discards empty lines, and drops the header (first remaining line)", async () => {
    await switchScriptToInline(wrapper);
    await switchCsvToInline(wrapper);

    const textareas = wrapper.findAll("textarea");
    await textareas[0].setValue("print('x')");
    // Mix of blank lines, whitespace-only lines, and trailing/leading spaces.
    await textareas[1].setValue(
      "\n   \n  name,age  \n Alice,30 \n\n Bob,25 \n   \n",
    );

    await clickSubmit(wrapper);

    const emissions = wrapper.emitted("submit");
    expect(emissions).toHaveLength(1);
    const payload = emissions![0][0] as {
      script: string;
      csvRows: string[];
    };
    expect(payload).not.toHaveProperty("header");
    // First non-empty line ("name,age") is treated as header and dropped.
    expect(payload.csvRows).toEqual(["Alice,30", "Bob,25"]);
  });

  // ---- Validation: row-count --------------------------------------

  it("rejects CSV with 0 data rows with 'CSV must contain at least 1 data row'", async () => {
    await switchScriptToInline(wrapper);
    await switchCsvToInline(wrapper);

    const textareas = wrapper.findAll("textarea");
    await textareas[0].setValue("print('x')");
    await textareas[1].setValue("only,header");

    await clickSubmit(wrapper);

    expect(wrapper.emitted("submit")).toBeFalsy();
    const formError = wrapper.find(".form-error").text();
    expect(formError).toBe("CSV must contain at least 1 data row");
  });

  it("rejects CSV with >3 data rows with 'dryRun requires 1 to 3 CSV rows'", async () => {
    await switchScriptToInline(wrapper);
    await switchCsvToInline(wrapper);

    const textareas = wrapper.findAll("textarea");
    await textareas[0].setValue("print('x')");
    await textareas[1].setValue("h1,h2\nr1a,r1b\nr2a,r2b\nr3a,r3b\nr4a,r4b");

    await clickSubmit(wrapper);

    expect(wrapper.emitted("submit")).toBeFalsy();
    const formError = wrapper.find(".form-error").text();
    expect(formError).toBe("dryRun requires 1 to 3 CSV rows");
  });

  // ---- Valid row counts: 1, 2, 3 ----------------------------------

  it("accepts exactly 1 data row and emits submit", async () => {
    await switchScriptToInline(wrapper);
    await switchCsvToInline(wrapper);

    const textareas = wrapper.findAll("textarea");
    await textareas[0].setValue("print('x')");
    await textareas[1].setValue("h1,h2\nonly,row");

    await clickSubmit(wrapper);

    const emissions = wrapper.emitted("submit");
    expect(emissions).toHaveLength(1);
    const payload = emissions![0][0] as {
      script: string;
      csvRows: string[];
    };
    expect(payload.csvRows).toEqual(["only,row"]);
  });

  it("accepts exactly 2 data rows and emits submit", async () => {
    await switchScriptToInline(wrapper);
    await switchCsvToInline(wrapper);

    const textareas = wrapper.findAll("textarea");
    await textareas[0].setValue("print('x')");
    await textareas[1].setValue("h1,h2\na,1\nb,2");

    await clickSubmit(wrapper);

    const emissions = wrapper.emitted("submit");
    expect(emissions).toHaveLength(1);
    const payload = emissions![0][0] as {
      script: string;
      csvRows: string[];
    };
    expect(payload.csvRows).toEqual(["a,1", "b,2"]);
  });

  it("accepts exactly 3 data rows and emits submit", async () => {
    await switchScriptToInline(wrapper);
    await switchCsvToInline(wrapper);

    const textareas = wrapper.findAll("textarea");
    await textareas[0].setValue("print('x')");
    await textareas[1].setValue("h1,h2\na,1\nb,2\nc,3");

    await clickSubmit(wrapper);

    const emissions = wrapper.emitted("submit");
    expect(emissions).toHaveLength(1);
    const payload = emissions![0][0] as {
      script: string;
      csvRows: string[];
    };
    expect(payload.csvRows).toEqual(["a,1", "b,2", "c,3"]);
  });
});
