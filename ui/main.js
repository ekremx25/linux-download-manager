const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

let currentMetadata = null;

document.addEventListener("DOMContentLoaded", async () => {
    setupEventListeners();
    await loadSettings();
    await loadDownloads();
    listenForUpdates();
});

function setupEventListeners() {
    document.getElementById("inspect-btn").addEventListener("click", inspectUrl);
    document.getElementById("start-btn").addEventListener("click", startDownload);
    document.getElementById("pick-dir-btn").addEventListener("click", pickDirectory);
    document.getElementById("toggle-settings").addEventListener("click", toggleSettings);
    document.getElementById("save-settings").addEventListener("click", saveSettings);
    document.getElementById("clear-completed-btn").addEventListener("click", clearCompleted);

    document.getElementById("url-input").addEventListener("keydown", (e) => {
        if (e.key === "Enter") inspectUrl();
    });
}

async function inspectUrl() {
    const url = document.getElementById("url-input").value.trim();
    if (!url) return;

    const btn = document.getElementById("inspect-btn");
    btn.textContent = "İnceleniyor...";
    btn.disabled = true;

    try {
        const metadata = await invoke("inspect_url", { url });
        currentMetadata = metadata;
        showMetadata(metadata);
    } catch (error) {
        alert("URL incelenemedi: " + error);
    } finally {
        btn.textContent = "İncele";
        btn.disabled = false;
    }
}

function showMetadata(metadata) {
    const panel = document.getElementById("metadata-panel");
    panel.hidden = false;

    document.getElementById("meta-filename").textContent = metadata.suggestedFileName;
    document.getElementById("meta-size").textContent = metadata.contentLength
        ? formatBytes(metadata.contentLength)
        : "Bilinmiyor";
    document.getElementById("meta-type").textContent = metadata.contentType || "Bilinmiyor";
    document.getElementById("meta-resumable").textContent = metadata.resumable ? "Evet" : "Hayır";
}

async function startDownload() {
    const url = document.getElementById("url-input").value.trim();
    if (!url) return;

    const saveDir = document.getElementById("save-dir-input").value.trim() || null;
    const expectedChecksum = document.getElementById("checksum-input").value.trim() || null;
    const bandwidthInput = document.getElementById("bandwidth-input").value;
    const bandwidthLimitKbps = bandwidthInput ? parseInt(bandwidthInput) : null;
    const scheduledAt = document.getElementById("schedule-input").value || null;

    try {
        await invoke("start_download", {
            url,
            saveDir,
            expectedChecksum,
            scheduledAt,
            bandwidthLimitKbps,
        });

        document.getElementById("url-input").value = "";
        document.getElementById("metadata-panel").hidden = true;
        document.getElementById("checksum-input").value = "";
        document.getElementById("bandwidth-input").value = "";
        document.getElementById("schedule-input").value = "";
        currentMetadata = null;
    } catch (error) {
        alert("İndirme başlatılamadı: " + error);
    }
}

async function pickDirectory() {
    try {
        const path = await invoke("pick_save_directory");
        if (path) {
            document.getElementById("save-dir-input").value = path;
        }
    } catch (error) {
        console.error("Klasör seçilemedi:", error);
    }
}

function toggleSettings() {
    const panel = document.getElementById("settings-panel");
    panel.hidden = !panel.hidden;
}

async function loadSettings() {
    try {
        const settings = await invoke("app_settings");
        document.getElementById("settings-download-dir").textContent = settings.defaultDownloadDir;
        document.getElementById("settings-max-concurrent").value = settings.maxConcurrentDownloads;
        document.getElementById("settings-bandwidth-limit").value = settings.defaultBandwidthLimitKbps || "";
    } catch (error) {
        console.error("Ayarlar yüklenemedi:", error);
    }
}

async function saveSettings() {
    const maxConcurrent = parseInt(document.getElementById("settings-max-concurrent").value);
    const bandwidthLimit = parseInt(document.getElementById("settings-bandwidth-limit").value) || 0;

    try {
        await invoke("update_app_settings", {
            maxConcurrentDownloads: maxConcurrent,
            defaultBandwidthLimitKbps: bandwidthLimit,
        });
        toggleSettings();
    } catch (error) {
        alert("Ayarlar kaydedilemedi: " + error);
    }
}

async function clearCompleted() {
    try {
        const count = await invoke("clear_completed");
        await loadDownloads();
    } catch (error) {
        alert("Temizleme başarısız: " + error);
    }
}

async function loadDownloads() {
    try {
        const downloads = await invoke("list_downloads");
        renderDownloads(downloads);
    } catch (error) {
        console.error("İndirmeler yüklenemedi:", error);
    }
}

function listenForUpdates() {
    listen("download://state", (event) => {
        updateDownloadItem(event.payload);
    });

    setInterval(loadDownloads, 5000);
}

function renderDownloads(downloads) {
    const container = document.getElementById("downloads-container");

    if (downloads.length === 0) {
        container.innerHTML = '<p class="empty-message">Henüz indirme yok.</p>';
        return;
    }

    container.innerHTML = downloads.map((dl) => renderDownloadItem(dl)).join("");
    attachDownloadActions();
    updateQueueSummary(downloads);
}

function renderDownloadItem(dl) {
    const progress = dl.totalBytes
        ? Math.min(100, (dl.downloadedBytes / dl.totalBytes) * 100).toFixed(1)
        : 0;
    const sizeText = dl.totalBytes
        ? `${formatBytes(dl.downloadedBytes)} / ${formatBytes(dl.totalBytes)}`
        : formatBytes(dl.downloadedBytes);

    let actions = "";
    if (dl.status === "in_progress" || dl.status === "queued") {
        actions += `<button class="btn btn-small btn-secondary" data-action="pause" data-id="${dl.id}">Duraklat</button>`;
        actions += `<button class="btn btn-small btn-danger" data-action="cancel" data-id="${dl.id}">İptal</button>`;
    } else if (dl.status === "paused" || dl.status === "failed") {
        actions += `<button class="btn btn-small btn-primary" data-action="resume" data-id="${dl.id}">Devam</button>`;
        actions += `<button class="btn btn-small btn-danger" data-action="cancel" data-id="${dl.id}">İptal</button>`;
    }

    let checksumHtml = "";
    if (dl.checksumStatus === "verified") {
        checksumHtml = '<span class="checksum-verified">✓ Checksum doğrulandı</span>';
    } else if (dl.checksumStatus === "mismatch") {
        checksumHtml = '<span class="checksum-mismatch">✗ Checksum uyuşmuyor</span>';
    }

    let errorHtml = "";
    if (dl.errorMessage && dl.status === "failed") {
        errorHtml = `<div class="error-text">${escapeHtml(dl.errorMessage)}</div>`;
    }

    return `
        <div class="download-item" data-download-id="${dl.id}">
            <div class="download-item-header">
                <span class="download-item-name">${escapeHtml(dl.fileName)}</span>
                <span class="download-item-status status-${dl.status}">${dl.status}</span>
            </div>
            <div class="download-progress">
                <div class="progress-bar">
                    <div class="progress-fill" style="width: ${progress}%"></div>
                </div>
            </div>
            <div class="download-item-details">
                <span>${sizeText}</span>
                <span>${progress}%</span>
            </div>
            ${checksumHtml}
            ${errorHtml}
            <div class="download-item-actions">${actions}</div>
        </div>
    `;
}

function attachDownloadActions() {
    document.querySelectorAll("[data-action]").forEach((btn) => {
        btn.addEventListener("click", async () => {
            const action = btn.dataset.action;
            const id = parseInt(btn.dataset.id);
            try {
                await invoke(`${action}_download`, { id });
                await loadDownloads();
            } catch (error) {
                alert(`İşlem başarısız: ${error}`);
            }
        });
    });
}

function updateDownloadItem(event) {
    const item = document.querySelector(`[data-download-id="${event.id}"]`);
    if (!item) {
        loadDownloads();
        return;
    }

    const statusEl = item.querySelector(".download-item-status");
    statusEl.textContent = event.status;
    statusEl.className = `download-item-status status-${event.status}`;

    if (event.totalBytes) {
        const progress = Math.min(100, (event.downloadedBytes / event.totalBytes) * 100).toFixed(1);
        const fill = item.querySelector(".progress-fill");
        if (fill) fill.style.width = `${progress}%`;

        const details = item.querySelector(".download-item-details");
        if (details) {
            let speedText = "";
            if (event.speedBytesPerSecond) {
                speedText = ` | ${formatBytes(event.speedBytesPerSecond)}/s`;
            }
            let etaText = "";
            if (event.etaSeconds) {
                etaText = ` | ETA: ${formatTime(event.etaSeconds)}`;
            }
            details.innerHTML = `
                <span>${formatBytes(event.downloadedBytes)} / ${formatBytes(event.totalBytes)}${speedText}${etaText}</span>
                <span>${progress}%</span>
            `;
        }
    }

    if (event.status === "completed" || event.status === "failed" || event.status === "cancelled") {
        loadDownloads();
    }
}

function updateQueueSummary(downloads) {
    const active = downloads.filter((d) => d.status === "in_progress").length;
    const pending = downloads.filter((d) => d.status === "queued" || d.status === "scheduled").length;
    const summary = document.getElementById("queue-summary");
    if (active || pending) {
        summary.textContent = `${active} aktif, ${pending} bekleyen`;
    } else {
        summary.textContent = "";
    }
}

function formatBytes(bytes) {
    if (bytes === 0) return "0 B";
    const units = ["B", "KB", "MB", "GB", "TB"];
    const i = Math.floor(Math.log(bytes) / Math.log(1024));
    return (bytes / Math.pow(1024, i)).toFixed(i > 0 ? 1 : 0) + " " + units[i];
}

function formatTime(seconds) {
    if (seconds < 60) return `${seconds}s`;
    if (seconds < 3600) return `${Math.floor(seconds / 60)}m ${seconds % 60}s`;
    return `${Math.floor(seconds / 3600)}h ${Math.floor((seconds % 3600) / 60)}m`;
}

function escapeHtml(text) {
    const div = document.createElement("div");
    div.textContent = text;
    return div.innerHTML;
}
