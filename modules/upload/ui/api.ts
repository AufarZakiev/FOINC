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

/**
 * Delete a previously-uploaded job.
 *
 * Calls `DELETE /api/jobs/{id}` (proxied to backend `DELETE /jobs/{id}`).
 * Resolves on `204 No Content`. Throws on any non-204 response (including
 * 404 and 500) with a message extracted from the response body when
 * available.
 */
export async function deleteJob(id: string): Promise<void> {
  const response = await fetch(`/api/jobs/${id}`, {
    method: "DELETE",
  });

  if (response.status === 204) {
    return;
  }

  const body = await response.json().catch(() => ({ error: "Delete failed" }));
  throw new Error(body.error ?? `Delete failed with status ${response.status}`);
}
