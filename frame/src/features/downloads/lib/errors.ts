import type { DownloadError } from "$shared/ipc/bindings";
import * as m from "$shared/i18n/messages";

export const downloadErrors: Record<DownloadError, () => string> = {
  invalid: m.download_error_invalid,
  unavailable: m.download_error_unavailable,
  unsupported: m.download_error_unsupported,
  capacity: m.download_error_capacity,
  storage: m.download_error_storage,
  destination: m.download_error_destination,
  permission: m.download_error_permission,
  network: m.download_error_network,
  connection_lost: m.download_error_connection_lost,
  timeout: m.download_error_timeout,
  authentication: m.download_error_authentication,
  certificate: m.download_error_certificate,
  server: m.download_error_server,
  source: m.download_error_source,
  file_busy: m.download_error_file_busy,
  file_too_large: m.download_error_file_too_large,
  integrity: m.download_error_integrity,
  runtime: m.download_error_runtime,
  disk_full: m.download_error_disk_full,
  protection: m.download_error_protection,
  missing_file: m.download_error_missing,
  changed_file: m.download_error_changed,
  cancelled: m.download_cancelled,
};

export function downloadReason(error: DownloadError | null, resumable = false): string {
  switch (error) {
    case "permission":
      return m.download_reason_permission();
    case "destination":
      return m.download_reason_destination();
    case "disk_full":
      return m.download_reason_disk_full();
    case "network":
      return m.download_reason_network();
    case "connection_lost":
      return m.download_reason_connection_lost();
    case "timeout":
      return m.download_reason_timeout();
    case "authentication":
      return m.download_reason_authentication();
    case "certificate":
      return m.download_reason_certificate();
    case "server":
      return m.download_reason_server();
    case "source":
      return m.download_reason_source();
    case "file_busy":
      return m.download_reason_file_busy();
    case "file_too_large":
      return m.download_reason_file_too_large();
    case "integrity":
      return resumable ? m.download_reason_incomplete_response() : m.download_reason_integrity();
    case "runtime":
      return m.download_reason_runtime();
    case "protection":
      return m.download_reason_protection();
    default:
      return m.download_reason_other();
  }
}

export function downloadDetail(error: DownloadError, resumable = false): string {
  if (resumable)
    return error === "file_busy" ? m.download_resume_file_busy() : m.download_resume_help();
  return downloadErrors[error]();
}
