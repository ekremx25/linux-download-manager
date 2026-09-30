// Linux Download Manager — Production UI Controller

// Tauri IPC helpers
function getTauri() {
    return typeof window !== "undefined" ? window.__TAURI__ : null;
}

async function invoke(cmd, args = {}) {
    const tauri = getTauri();
    if (tauri?.core?.invoke) {
        return await tauri.core.invoke(cmd, args);
    }
    // Fallback if accessed via pure static browser without Tauri
    if (cmd === "list_downloads") return [];
    if (cmd === "app_settings") {
        return {
            defaultDownloadDir: "~/Downloads",
            maxConcurrentDownloads: 3,
            defaultBandwidthLimitKbps: 0,
        };
    }
    return {};
}

function listen(evt, callback) {
    const tauri = getTauri();
    if (tauri?.event?.listen) {
        return tauri.event.listen(evt, callback);
    }
    return Promise.resolve(() => {});
}

// Application State
let allDownloads = [];
const liveDownloadStats = new Map();
let currentFilter = "all";
let currentCategory = "all";
let searchQuery = "";
let sortBy = "newest";
let appSettings = null;
let currentMetadata = null;
let deleteCandidateId = null;

// Initialize on DOM Ready
document.addEventListener("DOMContentLoaded", async () => {
    initEventListeners();
    await loadSettings();
    await loadDownloads();
    initRealtimeListeners();
    startStatsTicker();
});

// ── Event Listeners ─────────────────────────────────────────────────────────

function initEventListeners() {
    // Topbar
    document.getElementById("btn-new-download").addEventListener("click", openNewDownloadModal);
    document.getElementById("btn-open-settings").addEventListener("click", openSettingsModal);
    document.getElementById("btn-pause-all").addEventListener("click", togglePauseAll);

    // Sidebar status filters
    document.querySelectorAll(".sidebar-nav [data-filter]").forEach((btn) => {
        btn.addEventListener("click", () => {
            document.querySelectorAll(".sidebar-nav [data-filter]").forEach((b) => b.classList.remove("active"));
            btn.classList.add("active");
            currentFilter = btn.dataset.filter;
            currentCategory = "all";
            document.querySelectorAll(".sidebar-nav [data-category]").forEach((b) => b.classList.remove("active"));
            renderDownloadsList();
        });
    });

    // Sidebar category filters
    document.querySelectorAll(".sidebar-nav [data-category]").forEach((btn) => {
        btn.addEventListener("click", () => {
            document.querySelectorAll(".sidebar-nav [data-category]").forEach((b) => b.classList.remove("active"));
            btn.classList.add("active");
            currentCategory = btn.dataset.category;
            currentFilter = "all";
            document.querySelectorAll(".sidebar-nav [data-filter]").forEach((b) => b.classList.remove("active"));
            renderDownloadsList();
        });
    });

    // Open default download directory from sidebar
    document.getElementById("sidebar-open-folder-btn").addEventListener("click", async () => {
        if (appSettings?.defaultDownloadDir) {
            try {
                await invoke("open_folder", { path: appSettings.defaultDownloadDir });
                showToast("Opened download directory", "info");
            } catch (err) {
                showToast("Could not open folder: " + err, "error");
            }
        }
    });

    // Search and Sort
    const searchInput = document.getElementById("search-input");
    const clearSearchBtn = document.getElementById("search-clear-btn");

    searchInput.addEventListener("input", (e) => {
        searchQuery = e.target.value.trim().toLowerCase();
        clearSearchBtn.hidden = !searchQuery;
        renderDownloadsList();
    });

    clearSearchBtn.addEventListener("click", () => {
        searchInput.value = "";
        searchQuery = "";
        clearSearchBtn.hidden = true;
        renderDownloadsList();
    });

    document.getElementById("sort-select").addEventListener("change", (e) => {
        sortBy = e.target.value;
        renderDownloadsList();
    });

    document.getElementById("clear-completed-btn").addEventListener("click", clearCompletedDownloads);

    // New Download Modal
    document.getElementById("modal-close-btn").addEventListener("click", closeNewDownloadModal);
    document.getElementById("modal-cancel-btn").addEventListener("click", closeNewDownloadModal);
    document.getElementById("inspect-btn").addEventListener("click", inspectUrlInput);
    document.getElementById("paste-clipboard-btn").addEventListener("click", pasteFromClipboard);
    document.getElementById("pick-dir-btn").addEventListener("click", pickDirectory);
    document.getElementById("start-btn").addEventListener("click", startDownloadJob);

    document.getElementById("url-input").addEventListener("keydown", (e) => {
        if (e.key === "Enter") {
            e.preventDefault();
            if (currentMetadata) {
                startDownloadJob();
            } else {
                inspectUrlInput();
            }
        }
    });

    // Settings Modal
    document.getElementById("settings-close-btn").addEventListener("click", closeSettingsModal);
    document.getElementById("settings-cancel-btn").addEventListener("click", closeSettingsModal);
    document.getElementById("settings-browse-btn").addEventListener("click", pickDirectoryForSettings);
    document.getElementById("save-settings").addEventListener("click", saveSettings);
    document.getElementById("settings-max-concurrent").addEventListener("input", (e) => {
        document.getElementById("concurrent-display").textContent = e.target.value;
    });

    document.getElementById("btn-open-ext-folder").addEventListener("click", async () => {
        try {
            await invoke("open_folder", { path: `${appSettings?.defaultDownloadDir || ""}/../Documents/Linux Download Manager Extension` });
            showToast("Opened extension folder", "info");
        } catch (err) {
            showToast("Could not open extension folder", "error");
        }
    });

    // Delete Modal
    document.getElementById("delete-modal-close-btn").addEventListener("click", closeDeleteModal);
    document.getElementById("delete-cancel-btn").addEventListener("click", closeDeleteModal);
    document.getElementById("delete-confirm-btn").addEventListener("click", confirmDeleteDownload);

    // Keyboard Shortcuts
    window.addEventListener("keydown", (e) => {
        if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "n") {
            e.preventDefault();
            openNewDownloadModal();
        } else if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "f") {
            e.preventDefault();
            searchInput.focus();
        } else if (e.key === "Escape") {
            closeAllModals();
        }
    });
}

// ── Modals ──────────────────────────────────────────────────────────────────

async function openNewDownloadModal() {
    const modal = document.getElementById("new-download-modal");
    modal.hidden = false;
    const urlInput = document.getElementById("url-input");
    urlInput.focus();

    // Auto-paste if valid URL on clipboard
    try {
        if (navigator.clipboard?.readText) {
            const clipText = (await navigator.clipboard.readText()).trim();
            if (/^https?:\/\//i.test(clipText) && !urlInput.value) {
                urlInput.value = clipText;
                inspectUrlInput();
            }
        }
    } catch (_) {}
}

function closeNewDownloadModal() {
    document.getElementById("new-download-modal").hidden = true;
    document.getElementById("url-input").value = "";
    document.getElementById("custom-filename-input").value = "";
    document.getElementById("checksum-input").value = "";
    document.getElementById("bandwidth-input").value = "";
    document.getElementById("schedule-input").value = "";
    document.getElementById("metadata-panel").hidden = true;
    currentMetadata = null;
}

function openSettingsModal() {
    document.getElementById("settings-modal").hidden = false;
    loadSettings();
}

function closeSettingsModal() {
    document.getElementById("settings-modal").hidden = true;
}

function openDeleteModal(downloadId, fileName) {
    deleteCandidateId = downloadId;
    document.getElementById("delete-item-name").textContent = fileName;
    document.getElementById("delete-file-checkbox").checked = false;
    document.getElementById("delete-modal").hidden = false;
}

function closeDeleteModal() {
    deleteCandidateId = null;
    document.getElementById("delete-modal").hidden = true;
}

function closeAllModals() {
    closeNewDownloadModal();
    closeSettingsModal();
    closeDeleteModal();
}

async function pasteFromClipboard() {
    try {
        const text = (await navigator.clipboard.readText()).trim();
        if (text) {
            document.getElementById("url-input").value = text;
            inspectUrlInput();
        }
    } catch (_) {
        showToast("Clipboard access unavailable", "error");
    }
}

// ── URL Inspection & Start Download ─────────────────────────────────────────

async function inspectUrlInput() {
    const url = document.getElementById("url-input").value.trim();
    if (!url) return;

    const btn = document.getElementById("inspect-btn");
    btn.textContent = "Inspecting…";
    btn.disabled = true;

    try {
        const metadata = await invoke("inspect_url", { url });
        currentMetadata = metadata;
        showMetadataCard(metadata);
    } catch (error) {
        showToast("Could not inspect URL: " + error, "error");
    } finally {
        btn.textContent = "Inspect";
        btn.disabled = false;
    }
}

function showMetadataCard(meta) {
    const panel = document.getElementById("metadata-panel");
    panel.hidden = false;

    document.getElementById("custom-filename-input").value = meta.suggestedFileName || "";
    document.getElementById("meta-size").textContent = meta.contentLength
        ? formatBytes(meta.contentLength)
        : "Unknown / Stream";
    document.getElementById("meta-type").textContent = meta.contentType || "Binary";
    document.getElementById("meta-resumable").textContent = meta.resumable ? "Yes (Multi-threaded)" : "No";

    const badge = document.getElementById("meta-resumable-badge");
    badge.textContent = meta.resumable ? "Resumable: Yes" : "Resumable: No";
    badge.style.color = meta.resumable ? "var(--status-success)" : "var(--status-warning)";
    badge.style.background = meta.resumable ? "var(--status-success-bg)" : "var(--status-warning-bg)";
}

async function startDownloadJob() {
    const url = document.getElementById("url-input").value.trim();
    if (!url) {
        showToast("Please enter a download URL", "error");
        return;
    }

    const customFileName = document.getElementById("custom-filename-input").value.trim() || null;
    const saveDir = document.getElementById("save-dir-input").value.trim() || null;
    const expectedChecksum = document.getElementById("checksum-input").value.trim() || null;
    const bandwidthVal = document.getElementById("bandwidth-input").value;
    const bandwidthLimitKbps = bandwidthVal ? parseInt(bandwidthVal) : null;
    const scheduledAt = document.getElementById("schedule-input").value || null;

    const startBtn = document.getElementById("start-btn");
    startBtn.disabled = true;

    try {
        await invoke("start_download", {
            url,
            saveDir,
            expectedChecksum,
            scheduledAt,
            bandwidthLimitKbps,
            customFileName,
        });

        showToast("Download started", "success");
        closeNewDownloadModal();
        await loadDownloads();
    } catch (error) {
        showToast("Could not start download: " + error, "error");
    } finally {
        startBtn.disabled = false;
    }
}

async function pickDirectory() {
    try {
        const path = await invoke("pick_save_directory");
        if (path) {
            document.getElementById("save-dir-input").value = path;
        }
    } catch (error) {
        console.error("Could not pick folder:", error);
    }
}

async function pickDirectoryForSettings() {
    try {
        const path = await invoke("pick_save_directory");
        if (path) {
            document.getElementById("settings-download-dir").textContent = path;
        }
    } catch (error) {
        console.error("Could not pick folder:", error);
    }
}

// ── Application Settings ────────────────────────────────────────────────────

async function loadSettings() {
    try {
        appSettings = await invoke("app_settings");
        if (appSettings) {
            document.getElementById("settings-download-dir").textContent = appSettings.defaultDownloadDir || "~/Downloads";
            document.getElementById("sidebar-download-dir").textContent = appSettings.defaultDownloadDir || "~/Downloads";
            document.getElementById("settings-max-concurrent").value = appSettings.maxConcurrentDownloads || 3;
            document.getElementById("concurrent-display").textContent = appSettings.maxConcurrentDownloads || 3;
            document.getElementById("settings-bandwidth-limit").value = appSettings.defaultBandwidthLimitKbps || "";
        }
    } catch (error) {
        console.error("Could not load settings:", error);
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
        showToast("Settings saved", "success");
        closeSettingsModal();
        await loadSettings();
    } catch (error) {
        showToast("Could not save settings: " + error, "error");
    }
}

// ── Downloads Data Loading ──────────────────────────────────────────────────

async function loadDownloads() {
    try {
        const list = await invoke("list_downloads");
        allDownloads = Array.isArray(list) ? list : [];
        updateCounts();
        renderDownloadsList();
    } catch (error) {
        console.error("Could not load downloads:", error);
        allDownloads = [];
        updateCounts();
        renderDownloadsList();
    }
}

async function clearCompletedDownloads() {
    try {
        const count = await invoke("clear_completed");
        showToast(`Cleared ${count} finished item${count === 1 ? "" : "s"}`, "info");
        await loadDownloads();
    } catch (error) {
        showToast("Could not clear list: " + error, "error");
    }
}

async function confirmDeleteDownload() {
    if (!deleteCandidateId) return;
    const deleteFile = document.getElementById("delete-file-checkbox").checked;
    const id = deleteCandidateId;

    try {
        await invoke("delete_download", { id, deleteFile });
        showToast(deleteFile ? "Deleted download and file" : "Removed download from list", "info");
        closeDeleteModal();
        await loadDownloads();
    } catch (err) {
        showToast("Could not delete download: " + err, "error");
    }
}

async function togglePauseAll() {
    const hasActive = allDownloads.some((d) => d.status === "in_progress");
    const hasPaused = allDownloads.some((d) => d.status === "paused");

    if (hasActive) {
        for (const dl of allDownloads.filter((d) => d.status === "in_progress")) {
            try { await invoke("pause_download", { id: dl.id }); } catch (_) {}
        }
        showToast("Paused all downloads", "info");
    } else if (hasPaused) {
        for (const dl of allDownloads.filter((d) => d.status === "paused")) {
            try { await invoke("resume_download", { id: dl.id }); } catch (_) {}
        }
        showToast("Resumed downloads", "info");
    }
    await loadDownloads();
}

// ── Rendering Download Items ────────────────────────────────────────────────

function renderDownloadsList() {
    const container = document.getElementById("downloads-container");

    // Filter
    let filtered = allDownloads.filter((dl) => {
        if (currentFilter !== "all" && dl.status !== currentFilter) return false;
        if (currentCategory !== "all") {
            const cat = categorizeFile(dl.fileName);
            if (cat !== currentCategory) return false;
        }
        if (searchQuery) {
            const nameMatch = dl.fileName.toLowerCase().includes(searchQuery);
            const urlMatch = dl.url.toLowerCase().includes(searchQuery);
            if (!nameMatch && !urlMatch) return false;
        }
        return true;
    });

    // Sort
    filtered.sort((a, b) => {
        if (sortBy === "newest") return b.id - a.id;
        if (sortBy === "oldest") return a.id - b.id;
        if (sortBy === "name") return a.fileName.localeCompare(b.fileName);
        if (sortBy === "size") return (b.totalBytes || b.downloadedBytes || 0) - (a.totalBytes || a.downloadedBytes || 0);
        return 0;
    });

    if (filtered.length === 0) {
        container.innerHTML = `
            <div class="empty-state">
                <div class="empty-state-icon">
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round">
                        <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"></path>
                        <polyline points="7 10 12 15 17 10"></polyline>
                        <line x1="12" y1="15" x2="12" y2="3"></line>
                    </svg>
                </div>
                <div class="empty-state-title">${searchQuery ? "No matching downloads" : "No downloads in this view"}</div>
                <div class="empty-state-desc">${searchQuery ? "Try a different search keyword." : "Click New Download to start downloading."}</div>
                ${!searchQuery ? `<button class="btn btn-primary" onclick="openNewDownloadModal()">+ New Download</button>` : ""}
            </div>
        `;
        return;
    }

    container.innerHTML = filtered.map((dl) => renderDownloadCard(dl)).join("");
    attachCardActionHandlers();
}

// Rolling client-side download speed tracker (IDM-style real-time smoothing)
const clientDownloadTrackers = new Map();

function getComputedDownloadSpeedAndEta(id, downloaded, total, backendSpeed, backendEta) {
    const now = Date.now();
    let tracker = clientDownloadTrackers.get(id);

    if (!tracker) {
        tracker = {
            lastBytes: downloaded,
            lastTime: now,
            speed: backendSpeed || 0,
            smoothedSpeed: backendSpeed || 0
        };
        clientDownloadTrackers.set(id, tracker);
        const initialSpeed = backendSpeed || 0;
        return {
            speed: initialSpeed,
            eta: backendEta || (initialSpeed > 0 && total && downloaded < total ? Math.round((total - downloaded) / initialSpeed) : null)
        };
    }

    const elapsedMs = now - tracker.lastTime;
    let speed = 0;

    if (backendSpeed && backendSpeed > 0) {
        speed = backendSpeed;
        tracker.smoothedSpeed = tracker.smoothedSpeed > 0
            ? Math.round(tracker.smoothedSpeed * 0.65 + backendSpeed * 0.35)
            : backendSpeed;
        tracker.lastBytes = downloaded;
        tracker.lastTime = now;
        speed = tracker.smoothedSpeed;
    } else if (elapsedMs >= 350) {
        const deltaBytes = Math.max(0, downloaded - tracker.lastBytes);
        const instantSpeed = Math.round((deltaBytes / elapsedMs) * 1000);
        if (instantSpeed > 0) {
            tracker.smoothedSpeed = tracker.smoothedSpeed > 0
                ? Math.round(tracker.smoothedSpeed * 0.65 + instantSpeed * 0.35)
                : instantSpeed;
            speed = tracker.smoothedSpeed;
        } else {
            speed = tracker.smoothedSpeed || 0;
        }
        tracker.lastBytes = downloaded;
        tracker.lastTime = now;
    } else {
        speed = tracker.smoothedSpeed || 0;
    }

    let eta = backendEta;
    if ((!eta || eta <= 0) && speed > 0 && total && downloaded < total) {
        eta = Math.round((total - downloaded) / speed);
    }

    return { speed, eta };
}

function renderDownloadCard(dl) {
    const liveStats = liveDownloadStats.get(dl.id);
    const downloaded = liveStats?.downloadedBytes ?? dl.downloadedBytes;
    const total = liveStats?.totalBytes ?? dl.totalBytes;
    const isDownloading = dl.status === "in_progress";

    const { speed, eta } = isDownloading
        ? getComputedDownloadSpeedAndEta(
            dl.id,
            downloaded,
            total,
            liveStats?.speedBytesPerSecond ?? dl.speedBytesPerSecond,
            liveStats?.etaSeconds ?? dl.etaSeconds
        )
        : { speed: 0, eta: null };

    if (isDownloading && speed > 0) {
        liveDownloadStats.set(dl.id, {
            speedBytesPerSecond: speed,
            etaSeconds: eta,
            downloadedBytes: downloaded,
            totalBytes: total,
        });
    }

    const progress = total ? Math.min(100, (downloaded / total) * 100).toFixed(1) : 0;
    const category = categorizeFile(dl.fileName);

    let host = "";
    try {
        host = new URL(dl.url).hostname.replace(/^www\./, "");
    } catch (_) {
        host = "web";
    }

    // Size text
    let sizeText = total
        ? `${formatBytes(downloaded)} / ${formatBytes(total)}`
        : formatBytes(downloaded);

    // Speed text
    let speedHtml = "";
    if (isDownloading && speed > 0) {
        speedHtml = `
            <span class="metric-divider">·</span>
            <span class="metric-speed">${formatBytes(speed)}/s</span>
        `;
    }

    // ETA text
    let etaHtml = "";
    if (isDownloading && eta && eta > 0) {
        etaHtml = `
            <span class="metric-divider">·</span>
            <span class="metric-eta">${formatTime(eta)} left</span>
        `;
    }

    // Checksum indicator
    let checksumHtml = "";
    if (dl.checksumStatus === "verified") {
        checksumHtml = `<span class="metric-checksum verified">✓ Checksum OK</span>`;
    } else if (dl.checksumStatus === "mismatch") {
        checksumHtml = `<span class="metric-checksum mismatch">✗ Checksum Mismatch</span>`;
    }

    // Action buttons
    let actions = "";
    if (dl.status === "completed") {
        actions = `
            <button class="btn btn-secondary btn-sm" data-action="open-file" data-path="${escapeAttr(dl.savePath)}" title="Open file">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><polygon points="5 3 19 12 5 21 5 3"></polygon></svg>
                <span>Open</span>
            </button>
            <button class="btn btn-secondary btn-sm" data-action="open-folder" data-path="${escapeAttr(dl.savePath)}" title="Open enclosing folder">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"></path></svg>
                <span>Folder</span>
            </button>
            <button class="btn btn-ghost btn-sm btn-icon-only" data-action="copy-link" data-url="${escapeAttr(dl.url)}" title="Copy download URL">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><rect x="9" y="9" width="13" height="13" rx="2" ry="2"></rect><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"></path></svg>
            </button>
            <button class="btn btn-ghost btn-sm btn-icon-only" data-action="delete" data-id="${dl.id}" data-name="${escapeAttr(dl.fileName)}" title="Remove download">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><polyline points="3 6 5 6 21 6"></polyline><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"></path></svg>
            </button>
        `;
    } else if (isDownloading || dl.status === "queued") {
        actions = `
            <button class="btn btn-secondary btn-sm" data-action="pause" data-id="${dl.id}" title="Pause download">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><rect x="6" y="4" width="4" height="16"></rect><rect x="14" y="4" width="4" height="16"></rect></svg>
                <span>Pause</span>
            </button>
            <button class="btn btn-ghost btn-sm btn-icon-only" data-action="copy-link" data-url="${escapeAttr(dl.url)}" title="Copy link">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><rect x="9" y="9" width="13" height="13" rx="2" ry="2"></rect><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"></path></svg>
            </button>
            <button class="btn btn-danger btn-sm" data-action="cancel" data-id="${dl.id}" title="Cancel download">Cancel</button>
        `;
    } else {
        // Paused or Failed
        actions = `
            <button class="btn btn-primary btn-sm" data-action="resume" data-id="${dl.id}" title="Resume download">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><polygon points="5 3 19 12 5 21 5 3"></polygon></svg>
                <span>Resume</span>
            </button>
            <button class="btn btn-ghost btn-sm btn-icon-only" data-action="copy-link" data-url="${escapeAttr(dl.url)}" title="Copy link">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><rect x="9" y="9" width="13" height="13" rx="2" ry="2"></rect><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"></path></svg>
            </button>
            <button class="btn btn-ghost btn-sm btn-icon-only" data-action="delete" data-id="${dl.id}" data-name="${escapeAttr(dl.fileName)}" title="Remove">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><polyline points="3 6 5 6 21 6"></polyline><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"></path></svg>
            </button>
        `;
    }

    // Error Box
    let errorBox = "";
    if (dl.status === "failed" && dl.errorMessage) {
        errorBox = `<div class="card-error-box">${escapeHtml(dl.errorMessage)}</div>`;
    }

    return `
        <div class="download-card ${isDownloading ? "card-active" : ""}" data-download-id="${dl.id}">
            <div class="card-header">
                <div class="file-type-icon">
                    ${getCategorySvgIcon(category)}
                </div>
                <div class="card-title-group">
                    <div class="file-name-row">
                        <span class="file-name" title="${escapeAttr(dl.fileName)} (${escapeAttr(dl.savePath)})">${escapeHtml(dl.fileName)}</span>
                        <span class="domain-pill">${escapeHtml(host)}</span>
                    </div>
                </div>
                <div class="status-pill status-${dl.status}">
                    ${dl.status === "in_progress" ? `<span class="status-dot status-dot-active"></span>` : ""}
                    <span>${formatStatusLabel(dl.status)}</span>
                </div>
            </div>

            <div class="progress-container">
                <div class="progress-track">
                    <div class="progress-fill" style="width: ${progress}%"></div>
                </div>
            </div>

            <div class="card-footer">
                <div class="card-metrics">
                    <span class="metric-size">${sizeText}</span>
                    <span class="metric-percent">(${total ? `${progress}%` : "Live"})</span>
                    <span class="metric-speed-wrapper">${speedHtml}</span>
                    <span class="metric-eta-wrapper">${etaHtml}</span>
                    ${checksumHtml}
                </div>
                <div class="card-actions">
                    ${actions}
                </div>
            </div>
            ${errorBox}
        </div>
    `;
}

function attachCardActionHandlers() {
    document.querySelectorAll("[data-action]").forEach((btn) => {
        btn.addEventListener("click", async (e) => {
            e.stopPropagation();
            const action = btn.dataset.action;
            const id = btn.dataset.id ? parseInt(btn.dataset.id) : null;
            const path = btn.dataset.path;
            const url = btn.dataset.url;
            const name = btn.dataset.name;

            try {
                if (action === "pause") {
                    await invoke("pause_download", { id });
                    showToast("Paused", "info");
                    await loadDownloads();
                } else if (action === "resume") {
                    await invoke("resume_download", { id });
                    showToast("Resumed", "success");
                    await loadDownloads();
                } else if (action === "cancel") {
                    await invoke("cancel_download", { id });
                    showToast("Cancelled", "info");
                    await loadDownloads();
                } else if (action === "delete") {
                    openDeleteModal(id, name);
                } else if (action === "open-file") {
                    await invoke("open_file", { path });
                } else if (action === "open-folder") {
                    await invoke("open_folder", { path });
                } else if (action === "copy-link") {
                    await navigator.clipboard.writeText(url);
                    showToast("Link copied to clipboard", "info");
                }
            } catch (err) {
                showToast(`Action failed: ${err}`, "error");
            }
        });
    });
}

// ── Realtime IPC Updates & Stats Ticker ──────────────────────────────────────

let adaptivePollTimeout = null;

function scheduleAdaptivePoll() {
    clearTimeout(adaptivePollTimeout);
    const hasActive = allDownloads.some((d) => d.status === "in_progress");
    const interval = hasActive ? 800 : 3000;
    adaptivePollTimeout = setTimeout(async () => {
        await loadDownloads();
        scheduleAdaptivePoll();
    }, interval);
}

function initRealtimeListeners() {
    listen("download://state", (event) => {
        handleDownloadStateEvent(event.payload);
    });

    listen("tray-action", (event) => {
        if (event.payload === "add") {
            openNewDownloadModal();
        } else if (event.payload === "settings") {
            openSettingsModal();
        }
    });

    scheduleAdaptivePoll();
}

function handleDownloadStateEvent(event) {
    if (event.status === "in_progress") {
        const { speed, eta } = getComputedDownloadSpeedAndEta(
            event.id,
            event.downloadedBytes,
            event.totalBytes,
            event.speedBytesPerSecond,
            event.etaSeconds
        );
        liveDownloadStats.set(event.id, {
            speedBytesPerSecond: speed,
            etaSeconds: eta,
            downloadedBytes: event.downloadedBytes,
            totalBytes: event.totalBytes,
        });
    } else {
        liveDownloadStats.delete(event.id);
        clientDownloadTrackers.delete(event.id);
    }

    // Fast DOM update if card exists
    const card = document.querySelector(`[data-download-id="${event.id}"]`);
    if (card) {
        const total = event.totalBytes;
        const downloaded = event.downloadedBytes;
        const progress = total ? Math.min(100, (downloaded / total) * 100).toFixed(1) : 0;
        const fill = card.querySelector(".progress-fill");
        if (fill) fill.style.width = `${progress}%`;

        const metricsEl = card.querySelector(".card-metrics");
        if (metricsEl) {
            const sizeEl = metricsEl.querySelector(".metric-size");
            if (sizeEl) {
                sizeEl.textContent = total
                    ? `${formatBytes(downloaded)} / ${formatBytes(total)}`
                    : formatBytes(downloaded);
            }
            const pctEl = metricsEl.querySelector(".metric-percent");
            if (pctEl) {
                pctEl.textContent = `(${total ? `${progress}%` : "Live"})`;
            }

            const stats = liveDownloadStats.get(event.id);
            const speed = stats?.speedBytesPerSecond || 0;
            const speedWrap = metricsEl.querySelector(".metric-speed-wrapper");
            if (speedWrap) {
                speedWrap.innerHTML = speed > 0
                    ? `<span class="metric-divider">·</span><span class="metric-speed">${formatBytes(speed)}/s</span>`
                    : "";
            }

            const eta = stats?.etaSeconds;
            const etaWrap = metricsEl.querySelector(".metric-eta-wrapper");
            if (etaWrap) {
                etaWrap.innerHTML = (eta && eta > 0)
                    ? `<span class="metric-divider">·</span><span class="metric-eta">${formatTime(eta)} left</span>`
                    : "";
            }
        }

        if (event.status !== "in_progress") {
            loadDownloads();
        }
    } else {
        loadDownloads();
    }
}

function startStatsTicker() {
    setInterval(() => {
        let totalSpeed = 0;
        let activeCount = 0;
        let queuedCount = 0;

        for (const dl of allDownloads) {
            if (dl.status === "in_progress") {
                activeCount++;
                const stats = liveDownloadStats.get(dl.id);
                const s = stats?.speedBytesPerSecond ?? dl.speedBytesPerSecond ?? 0;
                if (s > 0) {
                    totalSpeed += s;
                }
            } else if (dl.status === "queued" || dl.status === "scheduled") {
                queuedCount++;
            }
        }

        const speedEl = document.getElementById("global-speed");
        const pulseEl = document.getElementById("speed-pulse");
        const queueEl = document.getElementById("global-queue-status");
        const pauseAllLabel = document.getElementById("label-pause-all");

        if (totalSpeed > 0) {
            speedEl.textContent = formatSpeed(totalSpeed);
            pulseEl.classList.add("active");
        } else {
            speedEl.textContent = "0.0 KB/s";
            pulseEl.classList.remove("active");
        }

        if (activeCount > 0 || queuedCount > 0) {
            queueEl.textContent = `${activeCount} active · ${queuedCount} queued`;
        } else {
            queueEl.textContent = "Idle";
        }

        if (activeCount > 0) {
            pauseAllLabel.textContent = "Pause All";
        } else if (allDownloads.some((d) => d.status === "paused")) {
            pauseAllLabel.textContent = "Resume All";
        } else {
            pauseAllLabel.textContent = "Pause All";
        }
    }, 500);
}

function updateCounts() {
    const counts = {
        all: allDownloads.length,
        in_progress: 0,
        queued: 0,
        paused: 0,
        completed: 0,
        failed: 0,
    };

    const catCounts = {
        video: 0,
        audio: 0,
        archive: 0,
        document: 0,
        image: 0,
    };

    for (const dl of allDownloads) {
        if (counts[dl.status] !== undefined) {
            counts[dl.status]++;
        } else if (dl.status === "scheduled") {
            counts.queued++;
        }

        const cat = categorizeFile(dl.fileName);
        if (catCounts[cat] !== undefined) {
            catCounts[cat]++;
        }
    }

    document.getElementById("count-all").textContent = counts.all;
    document.getElementById("count-in_progress").textContent = counts.in_progress;
    document.getElementById("count-queued").textContent = counts.queued;
    document.getElementById("count-paused").textContent = counts.paused;
    document.getElementById("count-completed").textContent = counts.completed;
    document.getElementById("count-failed").textContent = counts.failed;

    document.getElementById("cat-video").textContent = catCounts.video;
    document.getElementById("cat-audio").textContent = catCounts.audio;
    document.getElementById("cat-archive").textContent = catCounts.archive;
    document.getElementById("cat-document").textContent = catCounts.document;
    document.getElementById("cat-image").textContent = catCounts.image;
}

// ── File Categorization & Utilities ─────────────────────────────────────────

function categorizeFile(filename) {
    if (!filename || typeof filename !== "string") return "other";
    const parts = filename.toLowerCase().split(".");
    if (parts.length < 2) return "other";
    const ext = parts.pop();

    const videoExts = ["mp4", "mkv", "webm", "avi", "mov", "flv", "wmv", "m4v", "ts", "m3u8"];
    const audioExts = ["mp3", "m4a", "wav", "flac", "aac", "ogg", "opus", "wma"];
    const archiveExts = ["zip", "tar", "gz", "bz2", "xz", "7z", "rar", "deb", "rpm", "appimage", "iso", "bin", "apk", "exe"];
    const docExts = ["pdf", "doc", "docx", "txt", "rtf", "odt", "epub", "xls", "xlsx", "ppt", "pptx", "csv", "md"];
    const imgExts = ["jpg", "jpeg", "png", "gif", "webp", "svg", "bmp", "ico", "tiff"];

    if (videoExts.includes(ext)) return "video";
    if (audioExts.includes(ext)) return "audio";
    if (archiveExts.includes(ext)) return "archive";
    if (docExts.includes(ext)) return "document";
    if (imgExts.includes(ext)) return "image";
    return "other";
}

function getCategorySvgIcon(cat) {
    switch (cat) {
        case "video":
            return `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><polygon points="23 7 16 12 23 17 23 7"></polygon><rect x="1" y="5" width="15" height="14" rx="2" ry="2"></rect></svg>`;
        case "audio":
            return `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M9 18V5l12-2v13"></path><circle cx="6" cy="18" r="3"></circle><circle cx="18" cy="16" r="3"></circle></svg>`;
        case "archive":
            return `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M21 8v13H3V8"></path><path d="M1 3h22v5H1z"></path><line x1="10" y1="12" x2="14" y2="12"></line></svg>`;
        case "document":
            return `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"></path><polyline points="14 2 14 8 20 8"></polyline><line x1="16" y1="13" x2="8" y2="13"></line><line x1="16" y1="17" x2="8" y2="17"></line></svg>`;
        case "image":
            return `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><rect x="3" y="3" width="18" height="18" rx="2" ry="2"></rect><circle cx="8.5" cy="8.5" r="1.5"></circle><polyline points="21 15 16 10 5 21"></polyline></svg>`;
        default:
            return `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M13 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V9z"></path><polyline points="13 2 13 9 20 9"></polyline></svg>`;
    }
}

function formatStatusLabel(status) {
    switch (status) {
        case "in_progress": return "Downloading";
        case "completed": return "Completed";
        case "paused": return "Paused";
        case "queued": return "Queued";
        case "scheduled": return "Scheduled";
        case "failed": return "Failed";
        case "cancelled": return "Cancelled";
        default: return status;
    }
}

function formatBytes(bytes) {
    if (!bytes || bytes === 0) return "0 B";
    const units = ["B", "KB", "MB", "GB", "TB"];
    const i = Math.floor(Math.log(bytes) / Math.log(1024));
    return (bytes / Math.pow(1024, i)).toFixed(i > 0 ? 1 : 0) + " " + units[i];
}

function formatSpeed(bytesPerSecond) {
    const mbps = (bytesPerSecond * 8) / 1000000;
    return `${formatBytes(bytesPerSecond)}/s (${mbps.toFixed(1)} Mbps)`;
}

function formatTime(seconds) {
    if (!seconds || seconds <= 0) return "0s";
    if (seconds < 60) return `${Math.round(seconds)}s`;
    if (seconds < 3600) return `${Math.floor(seconds / 60)}m ${Math.round(seconds % 60)}s`;
    return `${Math.floor(seconds / 3600)}h ${Math.floor((seconds % 3600) / 60)}m`;
}

function escapeHtml(text) {
    if (!text) return "";
    const div = document.createElement("div");
    div.textContent = text;
    return div.innerHTML;
}

function escapeAttr(text) {
    if (!text) return "";
    return text.replace(/"/g, "&quot;").replace(/'/g, "&#39;");
}

// ── Toasts ──────────────────────────────────────────────────────────────────

function showToast(message, type = "info", duration = 2800) {
    const container = document.getElementById("toast-container");
    if (!container) return;

    const toast = document.createElement("div");
    toast.className = `toast toast-${type}`;

    let iconSvg = "";
    if (type === "success") {
        iconSvg = `<svg class="toast-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><polyline points="20 6 9 17 4 12"></polyline></svg>`;
    } else if (type === "error") {
        iconSvg = `<svg class="toast-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="12" cy="12" r="10"></circle><line x1="12" y1="8" x2="12" y2="12"></line><line x1="12" y1="16" x2="12.01" y2="16"></line></svg>`;
    } else {
        iconSvg = `<svg class="toast-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="12" cy="12" r="10"></circle><line x1="12" y1="16" x2="12" y2="12"></line><line x1="12" y1="8" x2="12.01" y2="8"></line></svg>`;
    }

    toast.innerHTML = `
        ${iconSvg}
        <span>${escapeHtml(message)}</span>
    `;

    container.appendChild(toast);

    setTimeout(() => {
        toast.style.opacity = "0";
        toast.style.transform = "translateY(6px)";
        toast.style.transition = "all 0.2s ease";
        setTimeout(() => toast.remove(), 200);
    }, duration);
}
