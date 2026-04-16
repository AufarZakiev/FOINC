import { describe, it, expect } from "vitest";
import { mount } from "@vue/test-utils";
import DryRunResults from "../DryRunResults.vue";
import type { DryRunResult } from "../dryRun";

function makeResult(
  rows: Array<{
    input: string;
    stdout: string;
    stderr: string;
    durationMs: number;
  }>,
): DryRunResult {
  return {
    rows,
    totalDurationMs: rows.reduce((a, r) => a + r.durationMs, 0),
  };
}

describe("DryRunResults.vue", () => {
  it("renders one <tr> in the tbody per entry in result.rows", () => {
    const result = makeResult([
      { input: "a,1", stdout: "o1", stderr: "", durationMs: 1 },
      { input: "b,2", stdout: "o2", stderr: "", durationMs: 2 },
      { input: "c,3", stdout: "o3", stderr: "", durationMs: 3 },
    ]);

    const wrapper = mount(DryRunResults, { props: { result } });
    const bodyRows = wrapper.findAll("tbody tr");
    expect(bodyRows).toHaveLength(3);
  });

  it("renders exactly one row when result.rows has one entry", () => {
    const result = makeResult([
      { input: "only,row", stdout: "hi", stderr: "", durationMs: 5 },
    ]);

    const wrapper = mount(DryRunResults, { props: { result } });
    const bodyRows = wrapper.findAll("tbody tr");
    expect(bodyRows).toHaveLength(1);
  });

  it("renders input, stdout, and stderr inside <pre> elements", () => {
    const result = makeResult([
      {
        input: "a,1",
        stdout: "the-stdout-value",
        stderr: "the-stderr-value",
        durationMs: 10,
      },
    ]);

    const wrapper = mount(DryRunResults, { props: { result } });
    const firstRow = wrapper.find("tbody tr");
    const pres = firstRow.findAll("pre");

    // Spec: input, stdout, stderr are each rendered inside a <pre>.
    expect(pres).toHaveLength(3);
    expect(pres[0].text()).toBe("a,1");
    expect(pres[1].text()).toBe("the-stdout-value");
    expect(pres[2].text()).toBe("the-stderr-value");
  });

  it("renders per-row durationMs with toFixed(2)", () => {
    const result = makeResult([
      { input: "a,1", stdout: "", stderr: "", durationMs: 12.345 },
      { input: "b,2", stdout: "", stderr: "", durationMs: 7 },
    ]);

    const wrapper = mount(DryRunResults, { props: { result } });
    const rows = wrapper.findAll("tbody tr");

    const dur0 = rows[0].find(".cell-duration").text();
    const dur1 = rows[1].find(".cell-duration").text();
    expect(dur0).toBe("12.35");
    expect(dur1).toBe("7.00");
  });

  it("renders totalDurationMs in the footer with toFixed(2) and a 'ms' suffix", () => {
    const result: DryRunResult = {
      rows: [
        { input: "a", stdout: "", stderr: "", durationMs: 1.111 },
        { input: "b", stdout: "", stderr: "", durationMs: 2.222 },
      ],
      totalDurationMs: 3.333,
    };

    const wrapper = mount(DryRunResults, { props: { result } });
    const footer = wrapper.find("tfoot");
    expect(footer.exists()).toBe(true);
    const footerText = footer.text();
    expect(footerText).toContain("3.33");
    expect(footerText).toContain("ms");
  });
});
