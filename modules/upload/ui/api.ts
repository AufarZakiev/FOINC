import type { Job } from "../../../integrations/ui/types";

/**
 * Upload a CSV file and a Python script to the backend.
 * Sends a multipart POST to /api/upload (proxied to backend /upload).
 */
export async function uploadFiles(csv: File, script: File): Promise<Job> {
  const formData = new FormData();
  formData.append("csv_file", csv);
  formData.append("script_file", script);

  const response = await fetch("/api/upload", {
    method: "POST",
    body: formData,
  });

  if (!response.ok) {
    const body = await response.json().catch(() => ({ error: "Upload failed" }));
    throw new Error(body.error ?? "Upload failed");
  }

  return response.json();
}
