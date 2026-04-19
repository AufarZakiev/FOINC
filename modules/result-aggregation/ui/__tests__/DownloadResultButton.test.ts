import { describe, it, expect } from "vitest";
import { mount } from "@vue/test-utils";

// Import the component under test. No module mocking is needed — the
// component has no imports other than the `JobStatus` type.
import DownloadResultButton from "../DownloadResultButton.vue";

// ---------------------------------------------------------------------------
// Tests: DownloadResultButton renders an <a download href> only when the
// job is in the `completed` state. For every non-completed status the
// component renders nothing (the `v-if` gates the whole template), so the
// wrapper finds no <a> element.
// ---------------------------------------------------------------------------

describe("DownloadResultButton.vue — conditional rendering", () => {
  it("test_renders_anchor_when_job_completed", () => {
    const wrapper = mount(DownloadResultButton, {
      props: { jobId: "abc", jobStatus: "completed" },
    });

    const anchor = wrapper.find("a");
    expect(anchor.exists()).toBe(true);
    // href points at the backend CSV stream for this job id.
    expect(anchor.attributes("href")).toBe("/api/jobs/abc/result");
    // The `download` attribute is present (value is empty string when the
    // attribute is written as a bare HTML boolean attribute).
    const downloadAttr = anchor.attributes("download");
    expect(downloadAttr).toBeDefined();
  });

  it("test_does_not_render_when_job_uploaded", () => {
    const wrapper = mount(DownloadResultButton, {
      props: { jobId: "abc", jobStatus: "uploaded" },
    });

    expect(wrapper.find("a").exists()).toBe(false);
  });

  it("test_does_not_render_when_job_processing", () => {
    const wrapper = mount(DownloadResultButton, {
      props: { jobId: "abc", jobStatus: "processing" },
    });

    expect(wrapper.find("a").exists()).toBe(false);
  });

  it("test_does_not_render_when_job_failed", () => {
    const wrapper = mount(DownloadResultButton, {
      props: { jobId: "abc", jobStatus: "failed" },
    });

    expect(wrapper.find("a").exists()).toBe(false);
  });
});
