const tauri = globalThis.window?.__TAURI__;

export const isTauri = Boolean(tauri?.core?.invoke);
export const invoke = tauri?.core?.invoke ?? (async () => { throw new Error("Tauri runtime is unavailable"); });
export const listen = tauri?.event?.listen ?? (async () => () => {});

export const STATUS_LABELS = {
  queued: "Added",
  scheduled: "Scheduled",
  in_progress: "Downloading",
  paused: "Paused",
  completed: "Finished",
  failed: "Failed",
  cancelled: "Cancelled",
  receiving: "Receiving data",
};

export function escapeHtml(value) {
  const div = document.createElement("div");
  div.textContent = value == null ? "" : String(value);
  return div.innerHTML;
}

export function formatBytes(bytes = 0) {
  if (!Number.isFinite(Number(bytes)) || Number(bytes) <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  const index = Math.min(units.length - 1, Math.floor(Math.log(Number(bytes)) / Math.log(1024)));
  return `${(Number(bytes) / 1024 ** index).toFixed(index ? 2 : 0)} ${units[index]}`;
}

export function formatSpeed(bytes = 0) {
  return bytes > 0 ? `${formatBytes(bytes)}/s` : "—";
}

export function formatTime(seconds) {
  const value = Number(seconds);
  if (!Number.isFinite(value) || value <= 0) return "—";
  if (value < 60) return `${Math.ceil(value)} s`;
  if (value < 3600) return `${Math.floor(value / 60)} m ${Math.ceil(value % 60)} s`;
  return `${Math.floor(value / 3600)} h ${Math.floor((value % 3600) / 60)} m`;
}

export function progressOf(download) {
  const total = Number(download?.totalBytes || 0);
  return total > 0 ? Math.max(0, Math.min(100, Number(download?.downloadedBytes || 0) / total * 100)) : 0;
}

// The backend classifies the real file name and stores it on the record, so
// this only renders it. The extension tables live in src-tauri/src/library.rs
// and are deliberately not duplicated here.
export function categoryFor(download) {
  if (typeof download === "string") return "other";
  return download?.category || "other";
}

export function categoryIcon(category) {
  return ({ image: "▧", music: "♫", video: "▰", apps: "✣", document: "▤", compressed: "▥", iso: "◉", other: "?" })[category] || "?";
}

export function statusLabel(status) {
  return STATUS_LABELS[status] || String(status || "Unknown").replaceAll("_", " ");
}

export function setNotice(element, message = "", type = "") {
  element.textContent = message;
  element.dataset.type = type;
  element.hidden = !message;
}
