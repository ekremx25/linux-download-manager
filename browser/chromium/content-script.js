const DOWNLOADABLE_EXTENSIONS = new Set([
  "7z",
  "apk",
  "appimage",
  "avi",
  "bin",
  "csv",
  "deb",
  "dmg",
  "epub",
  "exe",
  "flac",
  "gz",
  "img",
  "iso",
  "m4a",
  "mkv",
  "mov",
  "mp3",
  "mp4",
  "msi",
  "ogg",
  "pdf",
  "pkg",
  "rar",
  "rpm",
  "tar",
  "tgz",
  "torrent",
  "wav",
  "webm",
  "zip"
]);
const MEDIA_SOURCE_ATTRIBUTES = [
  "src",
  "data-src",
  "data-url",
  "data-video-src",
  "data-video-url",
  "data-stream",
  "data-stream-url",
  "data-hls",
  "data-mpd"
];
const IMAGE_EXTENSIONS = new Set(["avif", "bmp", "gif", "jpeg", "jpg", "png", "svg", "webp"]);
const MEDIA_CONTAINER_SELECTOR = [
  '[data-testid*="video" i]',
  '[data-testid*="media" i]',
  '[class*="video" i]',
  '[class*="player" i]',
  '[class*="media" i]',
  '[aria-label*="video" i]'
].join(", ");

let hoverButton = null;
let activeCandidate = null;
let reportTimer = null;
let recentMediaCandidates = [];
let mediaOverlayRoot = null;
let mediaRefreshTimer = null;
let toastRoot = null;
let toastTimer = null;
let captureTimeout = null;
let lastCaptureRequestAt = 0;
let lastLocationHref = window.location.href;
let periodicRefreshHandle = null;
let pageObserver = null;
const playerManifests = new Set();
let extensionDisconnected = false;

function disconnectExtension() {
  if (extensionDisconnected) return;
  extensionDisconnected = true;
  window.clearInterval(periodicRefreshHandle);
  window.clearTimeout(reportTimer);
  window.clearTimeout(mediaRefreshTimer);
  clearCaptureTimeout();
  pageObserver?.disconnect();
  hoverButton?.remove();
  mediaOverlayRoot?.remove();
  showToast("Extension reloaded. Please refresh this page.", "info");
}

function sendExtensionMessage(message, callback) {
  if (extensionDisconnected) return;
  try {
    if (!chrome.runtime?.id) { disconnectExtension(); return; }
    chrome.runtime.sendMessage(message, (response) => {
      const error = chrome.runtime.lastError;
      if (error && /context invalidated/i.test(error.message ?? "")) {
        disconnectExtension();
        return;
      }
      if (callback) callback(response);
    });
  } catch (error) {
    if (/context invalidated/i.test(error.message ?? "")) {
      disconnectExtension();
      return;
    }
    clearCaptureTimeout();
    showToast("Failed to send extension message: " + error.message, "error");
  }
}

bootstrap();

function isExcludedHost(hostname) {
  return /(^|\.)(whatsapp\.com|whatsapp\.net)$/i.test(hostname);
}

function bootstrap() {
  if (/(^|\.)molystream\.org$/i.test(window.location.hostname)) {
    window.addEventListener("message", (event) => {
      if (event.source !== window || event.data?.type !== "ldm-player-manifest") return;
      const url = normalizeUrl(event.data.url);
      if (!url || playerManifests.has(url)) return;
      playerManifests.add(url);
      if (playerManifests.size > 12) playerManifests.delete(playerManifests.values().next().value);
      reportObservedMediaCandidates();
    });
    window.postMessage({type: "ldm-request-player-manifests"}, window.location.origin);
  }
  if (isExcludedHost(window.location.hostname)) {
    return;
  }

  // Embedded players run this script too so their media can still be reported,
  // but only the top-level page should render controls. Otherwise every iframe
  // adds its own LDM button over the same visible player.
  if (window !== window.top) {
    scheduleCandidateReport();
    observePageChanges();
    reportObservedMediaCandidates();
    periodicRefreshHandle = window.setInterval(reportObservedMediaCandidates, 1200);
    document.addEventListener("loadedmetadata", handleMediaSignal, true);
    document.addEventListener("play", handleMediaSignal, true);
    return;
  }

  createHoverButton();
  createMediaOverlay();
  createToastRoot();
  refreshInlineDownloadButtons();
  scheduleCandidateReport();
  observePageChanges();
  refreshRecentMediaCandidates();
  scheduleMediaOverlayRefresh();
  startPeriodicRefresh();

  document.addEventListener("pointerover", handlePointerOver, true);
  document.addEventListener("pointermove", handlePointerOver, true);
  document.addEventListener("pointerdown", handlePointerDown, true);
  document.addEventListener("scroll", repositionHoverButton, true);
  document.addEventListener("scroll", scheduleMediaOverlayRefresh, true);
  window.addEventListener("resize", repositionHoverButton);
  window.addEventListener("resize", scheduleMediaOverlayRefresh);
  document.addEventListener("loadedmetadata", handleMediaSignal, true);
  document.addEventListener("play", handleMediaSignal, true);
  window.addEventListener("popstate", handleRouteChange);
  window.addEventListener("hashchange", handleRouteChange);
  document.addEventListener("yt-navigate-finish", () => detectLocationChange(true));

  chrome.runtime.onMessage.addListener((message) => {
    if (message?.type !== "native-capture-status") {
      return;
    }

    clearCaptureTimeout();
    showToast(
      message.message || "Linux Download Manager returned an unknown response.",
      message.tone || "info"
    );
  });
}

function createHoverButton() {
  // Sites with their own download control do not need a generic hover button.
  if (isYtdlpSupportedSite() || hasInlineButtons() || hoverButton) {
    return;
  }

  hoverButton = document.createElement("button");
  hoverButton.type = "button";
  hoverButton.className = "ldm-hover-button";
  hoverButton.textContent = "LDM";
  hoverButton.title = "Download with Linux Download Manager";
  hoverButton.hidden = true;
  hoverButton.addEventListener("click", handleHoverButtonClick);
  document.documentElement.appendChild(hoverButton);
}

function createMediaOverlay() {
  if (mediaOverlayRoot) {
    return;
  }

  mediaOverlayRoot = document.createElement("div");
  mediaOverlayRoot.className = "ldm-media-layer";
  document.documentElement.appendChild(mediaOverlayRoot);
}

function createToastRoot() {
  if (toastRoot) {
    return;
  }

  toastRoot = document.createElement("div");
  toastRoot.className = "ldm-toast-root";
  document.documentElement.appendChild(toastRoot);
}

function isCandidateAlreadyDecorated(element) {
  if (!element) return false;
  if (element.dataset?.ldmDecorated === "true") return true;
  if (element.querySelector?.(".ldm-site-btn, .ldm-inline-btn, .ldm-media-button, #ldm-yt-download-btn")) {
    return true;
  }
  let cur = element;
  for (let i = 0; i < 8 && cur; i++) {
    if (cur.dataset?.ldmDecorated === "true" || cur.querySelector?.(".ldm-site-btn, .ldm-inline-btn, .ldm-media-button, #ldm-yt-download-btn")) {
      return true;
    }
    cur = cur.parentElement;
  }
  return false;
}

function hasInlineButtons() {
  return (/youtube\.com|youtu\.be/i.test(window.location.hostname) && Boolean(document.querySelector("#ldm-yt-download-btn, .ldm-site-btn, .ldm-inline-btn"))) ||
    (/facebook\.com|fb\.watch|instagram\.com|x\.com|twitter\.com|reddit\.com|tiktok\.com/i.test(window.location.hostname) && Boolean(document.querySelector(".ldm-site-btn")));
}

function handlePointerOver(event) {
  if (isYtdlpSupportedSite()) {
    hideHoverButton();
    return;
  }

  const candidate = extractCandidateFromPoint(event);
  if (!candidate || candidate.kind === "media" || candidate.kind === "media-fallback" || isCandidateAlreadyDecorated(candidate.element)) {
    return;
  }

  activeCandidate = candidate;
  repositionHoverButton();
}

function handlePointerDown(event) {
  if (!hoverButton || hoverButton.hidden) {
    return;
  }

  if (hoverButton.contains(event.target)) {
    return;
  }

  if (isEventInsideActiveCandidate(event)) {
    return;
  }

  const candidate = extractCandidateFromPoint(event);
  if (!candidate || candidate.element !== activeCandidate?.element) {
    hideHoverButton();
  }
}

function handleHoverButtonClick(event) {
  event.preventDefault();
  event.stopPropagation();

  if (!activeCandidate) {
    return;
  }

  triggerCaptureForCandidate(activeCandidate, hoverButton);
}

function isYtdlpSupportedSite() {
  const host = window.location.hostname;
  return /(youtube\.com|youtu\.be|x\.com|twitter\.com|facebook\.com|instagram\.com|fb\.watch|reddit\.com|tiktok\.com|vimeo\.com|dailymotion\.com|twitch\.tv)$/i.test(host);
}

function showQualityPicker(candidate, anchorElement) {
  closeQualityPicker();

  const qualities = [
    { label: "Best quality", format: "bv*+ba/b" },
    { label: "4K (2160p)", format: "bv*[height<=2160]+ba/b" },
    { label: "1440p", format: "bv*[height<=1440]+ba/b" },
    { label: "1080p", format: "bv*[height<=1080]+ba/b" },
    { label: "720p", format: "bv*[height<=720]+ba/b" },
    { label: "480p", format: "bv*[height<=480]+ba/b" },
    { label: "360p", format: "bv*[height<=360]+ba/b" },
    { label: "Audio only", format: "ba/b" }
  ];

  const picker = document.createElement("div");
  picker.setAttribute("style", `
    position: fixed !important;
    z-index: 2147483647 !important;
    background: rgba(18, 18, 18, 0.97) !important;
    border: 1px solid rgba(255, 255, 255, 0.12) !important;
    border-radius: 12px !important;
    padding: 6px 0 !important;
    min-width: 180px !important;
    box-shadow: 0 20px 50px rgba(0,0,0,0.5) !important;
    display: block !important;
    visibility: visible !important;
    opacity: 1 !important;
    pointer-events: auto !important;
  `);
  picker.id = "ldm-quality-picker";

  const title = document.createElement("div");
  title.setAttribute("style", `
    padding: 10px 16px 6px !important;
    font: 700 13px/1 sans-serif !important;
    color: #86e8ff !important;
    letter-spacing: 0.04em !important;
  `);
  title.textContent = "Select Quality";
  picker.appendChild(title);

  for (const quality of qualities) {
    const btn = document.createElement("button");
    btn.type = "button";
    btn.textContent = quality.label;
    btn.setAttribute("style", `
      display: block !important;
      width: 100% !important;
      border: 0 !important;
      background: transparent !important;
      color: #e8e8e8 !important;
      font: 500 13px/1 sans-serif !important;
      padding: 10px 16px !important;
      text-align: left !important;
      cursor: pointer !important;
      pointer-events: auto !important;
    `);
    btn.addEventListener("pointerdown", (e) => { e.stopPropagation(); });
    btn.addEventListener("mouseenter", () => {
      btn.style.setProperty("background", "rgba(61,210,159,0.18)", "important");
      btn.style.setProperty("color", "#3dd29f", "important");
    });
    btn.addEventListener("mouseleave", () => {
      btn.style.setProperty("background", "transparent", "important");
      btn.style.setProperty("color", "#e8e8e8", "important");
    });
    btn.addEventListener("click", (event) => {
      event.preventDefault();
      event.stopPropagation();
      event.stopImmediatePropagation();
      closeQualityPicker();
      triggerCaptureWithFormat(candidate, quality.format);
    }, true);
    picker.appendChild(btn);
  }

  picker.style.setProperty("top", "50%", "important");
  picker.style.setProperty("left", "50%", "important");
  picker.style.setProperty("transform", "translate(-50%, -50%)", "important");

  document.documentElement.appendChild(picker);

  setTimeout(() => {
    document.addEventListener("pointerdown", handleQualityPickerOutsideClick, true);
  }, 100);
}

function closeQualityPicker() {
  document.removeEventListener("pointerdown", handleQualityPickerOutsideClick, true);
  const existing = document.getElementById("ldm-quality-picker");
  if (existing) existing.remove();
}

function handleQualityPickerOutsideClick(event) {
  const picker = document.getElementById("ldm-quality-picker");
  if (picker && !picker.contains(event.target)) {
    closeQualityPicker();
  }
}

function resolveSourcePageUrl(candidate) {
  if (candidate?.url && /^https?:\/\/.*(reddit\.com|x\.com|twitter\.com|youtube\.com|facebook\.com|instagram\.com|tiktok\.com|vimeo\.com|dailymotion\.com)\//.test(candidate.url)) {
    return candidate.url;
  }
  return window.location.href;
}

function triggerCaptureWithFormat(candidate, format) {
  showToast("Sending download to Linux Download Manager...", "info");
  armCaptureTimeout();

  const sourcePageUrl = resolveSourcePageUrl(candidate);

  if (candidate.kind === "media" || candidate.kind === "media-fallback") {
    sendExtensionMessage({
      type: "capture-best-media",
      payload: {
        preferredUrl: candidate.url ?? null,
        sourcePageUrl: sourcePageUrl,
        sourceTitle: candidate.title || document.title,
        format: format ?? null
      }
    }, (response) => {
      clearCaptureTimeout();
      if (chrome.runtime.lastError) {
        showToast(chrome.runtime.lastError.message, "error");
        return;
      }
      if (!response?.ok) {
        clearCaptureTimeout();
        showToast(response?.error ?? "Media capture failed.", "error");
        return;
      }
      pulseHoverButton();
    });
    return;
  }

  sendExtensionMessage({
    type: "capture-download",
    payload: {
      url: candidate.url,
      sourcePageUrl: sourcePageUrl,
      sourceTitle: document.title,
      format: format ?? null
    }
  }, (response) => {
    if (chrome.runtime.lastError) {
      clearCaptureTimeout();
      showToast(chrome.runtime.lastError.message, "error");
      return;
    }
    if (!response?.ok) {
      clearCaptureTimeout();
      showToast(response?.error ?? "Download request failed.", "error");
      return;
    }
    pulseHoverButton();
  });
}

function triggerCaptureForCandidate(candidate, anchorElement) {
  if (!candidate) {
    return;
  }

  if (isYtdlpSupportedSite()) {
    showQualityPicker(candidate, anchorElement);
    return;
  }

  triggerCaptureWithFormat(candidate, null);
}

function triggerCaptureOnce(candidate, anchorElement) {
  const now = Date.now();
  if (now - lastCaptureRequestAt < 2000) {
    return;
  }

  lastCaptureRequestAt = now;
  triggerCaptureForCandidate(candidate, anchorElement);
}

function pulseHoverButton() {
  if (!hoverButton) {
    return;
  }

  hoverButton.classList.add("ldm-hover-button-sent");
  window.setTimeout(() => {
    hoverButton?.classList.remove("ldm-hover-button-sent");
  }, 900);
}

function repositionHoverButton() {
  if (isYtdlpSupportedSite() || isStickyMediaCandidate(activeCandidate)) {
    hideHoverButton();
    return;
  }
  if (mediaOverlayRoot?.childElementCount > 0) {
    if (hoverButton) hoverButton.hidden = true;
    return;
  }
  if (!hoverButton || !activeCandidate) {
    hideHoverButton();
    return;
  }

  if (!activeCandidate.element?.isConnected) {
    hideHoverButton();
    return;
  }

  const rect = activeCandidate.element.getBoundingClientRect();
  if (rect.width <= 0 || rect.height <= 0) {
    hideHoverButton();
    return;
  }

  hoverButton.style.position = "absolute";
  hoverButton.style.right = "auto";
  const top = Math.max(8, rect.top + window.scrollY - 10);
  const left = Math.max(8, rect.right + window.scrollX - 44);
  hoverButton.style.top = `${top}px`;
  hoverButton.style.left = `${left}px`;
  hoverButton.hidden = false;
}

function hideHoverButton() {
  activeCandidate = null;
  if (hoverButton) {
    hoverButton.hidden = true;
  }
}

function isStickyMediaCandidate(candidate) {
  return candidate?.kind === "media" || candidate?.kind === "media-fallback";
}

function isEventInsideActiveCandidate(event) {
  if (!activeCandidate?.element?.isConnected) {
    return false;
  }

  if (typeof event.clientX !== "number" || typeof event.clientY !== "number") {
    return false;
  }

  const rect = activeCandidate.element.getBoundingClientRect();
  return (
    event.clientX >= rect.left &&
    event.clientX <= rect.right &&
    event.clientY >= rect.top &&
    event.clientY <= rect.bottom
  );
}

function observePageChanges() {
  const observer = pageObserver = new MutationObserver(() => {
    detectLocationChange();
    scheduleCandidateReport();
    scheduleMediaRefresh();
    scheduleMediaOverlayRefresh();
    refreshInlineDownloadButtons();
    if (activeCandidate && !activeCandidate.element.isConnected) {
      hideHoverButton();
    }
  });

  observer.observe(document.documentElement, {
    childList: true,
    subtree: true,
    attributes: true,
    attributeFilter: ["href", "src", "download"]
  });
}

function handleMediaSignal(event) {
  scheduleCandidateReport();
  scheduleMediaRefresh();
  scheduleMediaOverlayRefresh();
  refreshInlineDownloadButtons();
}

function handleRouteChange() {
  detectLocationChange(true);
}

function detectLocationChange(force = false) {
  if (!force && window.location.href === lastLocationHref) {
    return;
  }

  lastLocationHref = window.location.href;
  hideHoverButton();
  if (isYtdlpSupportedSite() && hoverButton) {
    hoverButton.hidden = true;
    hoverButton.remove();
    hoverButton = null;
  }
  scheduleCandidateReport();
  scheduleMediaRefresh();
  scheduleMediaOverlayRefresh();
  refreshInlineDownloadButtons();
}

function startPeriodicRefresh() {
  if (periodicRefreshHandle) {
    window.clearInterval(periodicRefreshHandle);
  }

  periodicRefreshHandle = window.setInterval(() => {
    detectLocationChange();
    scheduleMediaRefresh();
    scheduleMediaOverlayRefresh();
    refreshInlineDownloadButtons();
  }, 1200);
}

function scheduleMediaRefresh() {
  if (extensionDisconnected) return;
  window.setTimeout(() => {
    reportObservedMediaCandidates();
    refreshRecentMediaCandidates();
  }, 180);
}

function scheduleMediaOverlayRefresh() {
  if (extensionDisconnected) return;
  if (mediaRefreshTimer) {
    window.clearTimeout(mediaRefreshTimer);
  }

  mediaRefreshTimer = window.setTimeout(() => {
    mediaRefreshTimer = null;
    refreshMediaOverlay();
  }, 120);
}

function scheduleCandidateReport() {
  if (extensionDisconnected) return;
  if (reportTimer) {
    window.clearTimeout(reportTimer);
  }

  reportTimer = window.setTimeout(() => {
    reportTimer = null;
    sendExtensionMessage({
      type: "candidate-count",
      count: countCandidates()
    });
  }, 120);
}

function countCandidates() {
  const seen = new Set();
  let count = 0;

  for (const element of document.querySelectorAll("a[href], video, audio, source")) {
    const candidate = extractCandidateFromTarget(element);
    if (!candidate || seen.has(candidate.url)) {
      continue;
    }

    seen.add(candidate.url);
    count += 1;
  }

  return count;
}

function extractCandidateFromPoint(event) {
  const pointCandidates = [];

  if (typeof event.clientX === "number" && typeof event.clientY === "number") {
    for (const element of document.elementsFromPoint(event.clientX, event.clientY)) {
      pointCandidates.push(element);
    }

    for (const mediaElement of findVisibleMediaAtPoint(event.clientX, event.clientY)) {
      pointCandidates.push(mediaElement);
    }
  }

  pointCandidates.push(event.target);

  for (const target of pointCandidates) {
    const candidate = extractCandidateFromTarget(target);
    if (candidate) {
      return candidate;
    }
  }

  return null;
}

function extractCandidateFromTarget(target) {
  if (!(target instanceof Element)) {
    return null;
  }

  const link = target.closest("a[href]");
  if (link) {
    const url = normalizeUrl(link.getAttribute("href"));
    if (url && isDownloadableLink(link, url)) {
      return { element: link, url };
    }
  }

  const mediaTarget = target.closest("video, audio, source");
  if (mediaTarget) {
    const mediaElement = mediaTarget.tagName === "SOURCE" && mediaTarget.parentElement
      ? mediaTarget.parentElement
      : mediaTarget;
    const url = resolveMediaUrl(mediaTarget, mediaElement);
    if (url && isMediaCandidate(url)) {
      return { element: mediaElement, url, kind: "media" };
    }

    if (mediaElement instanceof HTMLMediaElement) {
      return {
        element: mediaElement,
        url: recentMediaCandidates[0] ?? null,
        kind: "media-fallback"
      };
    }
  }

  const mediaContainer = findMediaContainer(target);
  if (mediaContainer) {
    return extractCandidateFromMediaContainer(mediaContainer);
  }

  return null;
}

function refreshMediaOverlay() {
  if (!mediaOverlayRoot) {
    return;
  }

  refreshInlineDownloadButtons();

  // If on a supported site (YouTube, Facebook, Twitter, Instagram, Reddit, etc.),
  // direct on-player badges are used. Never render a duplicate floating overlay!
  if (isYtdlpSupportedSite()) {
    mediaOverlayRoot.replaceChildren();
    return;
  }

  // If any video on the page is already decorated with an on-player badge, do not float extra overlays
  const decorated = document.querySelector(".ldm-site-btn, [data-ldm-decorated='true']");
  if (decorated) {
    const visibleVideos = Array.from(document.querySelectorAll("video")).filter((v) => {
      const r = v.getBoundingClientRect();
      return r.width > 120 && r.height > 80 && isVisibleMediaRect(r);
    });
    if (visibleVideos.length > 0 && visibleVideos.every((v) => isCandidateAlreadyDecorated(v))) {
      mediaOverlayRoot.replaceChildren();
      return;
    }
  }

  const overlayTargets = collectOverlayTargets();
  const activeOverlayTarget = resolveActiveOverlayTarget();
  if (activeOverlayTarget && !overlayTargets.some((target) => target.element === activeOverlayTarget.element)) {
    overlayTargets.unshift(activeOverlayTarget);
  }

  const persistentCandidate = resolvePersistentMediaCandidate();
  if (
    persistentCandidate &&
    !overlayTargets.some((target) => isSameCandidate(target.candidate, persistentCandidate))
  ) {
    const persistentElement = resolveCandidateAnchorElement(persistentCandidate);
    overlayTargets.unshift(
      persistentElement
        ? {
            element: persistentElement,
            candidate: persistentCandidate
          }
        : {
            candidate: persistentCandidate,
            pinned: false
          }
    );
  }

  let validTarget = null;
  for (const target of overlayTargets) {
    if (isCandidateAlreadyDecorated(target.element)) {
      continue;
    }
    const rect = target.element?.getBoundingClientRect?.();
    if (rect && isVisibleMediaRect(rect)) {
      validTarget = { ...target, rect };
      break;
    }
  }

  if (!validTarget) {
    mediaOverlayRoot.replaceChildren();
    return;
  }

  let button = mediaOverlayRoot.querySelector(".ldm-media-button");
  if (!button) {
    button = document.createElement("button");
    button.type = "button";
    button.className = "ldm-media-button";
    button.innerHTML = `<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" style="display:inline-block;vertical-align:-1px;margin-right:5px;flex-shrink:0;"><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"></path><polyline points="7 10 12 15 17 10"></polyline><line x1="12" y1="15" x2="12" y2="3"></line></svg><span>Download</span>`;
    button.addEventListener("pointerdown", (event) => {
      event.preventDefault();
      event.stopPropagation();
      if (activeCandidate) {
        triggerCaptureOnce(activeCandidate, button);
      }
    });
    button.addEventListener("click", (event) => {
      event.preventDefault();
      event.stopPropagation();
    });
    mediaOverlayRoot.replaceChildren(button);
  }

  activeCandidate = validTarget.candidate;
  const rect = validTarget.rect;
  const top = Math.max(12, Math.min(rect.top + 12, window.innerHeight - 50));
  const right = Math.max(12, Math.min(window.innerWidth - rect.right + 12, window.innerWidth - 140));
  button.style.setProperty("top", `${top}px`, "important");
  button.style.setProperty("right", `${right}px`, "important");
  button.style.setProperty("left", "auto", "important");
  button.style.setProperty("transform", "none", "important");
  button.style.setProperty("display", "block", "important");

  if (hoverButton) hoverButton.hidden = true;
}

function createSiteVideoBadge(onClick, label = "Download") {
  const btn = document.createElement("div");
  btn.className = "ldm-site-btn";
  btn.setAttribute("role", "button");
  btn.setAttribute("tabindex", "0");
  btn.setAttribute("title", "Download with Linux Download Manager");
  btn.innerHTML = `<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" style="display:inline-block;vertical-align:-1px;margin-right:5px;flex-shrink:0;"><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"></path><polyline points="7 10 12 15 17 10"></polyline><line x1="12" y1="15" x2="12" y2="3"></line></svg><span>${label}</span>`;
  btn.setAttribute("style", `
    position: absolute !important;
    top: 10px !important;
    right: 10px !important;
    z-index: 2147483647 !important;
    background: linear-gradient(135deg, #10b981, #06b6d4) !important;
    color: #04110d !important;
    border: 0 !important;
    border-radius: 9999px !important;
    padding: 6px 13px !important;
    font: 700 12px/1 -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif !important;
    letter-spacing: 0.02em !important;
    cursor: pointer !important;
    pointer-events: auto !important;
    user-select: none !important;
    box-shadow: 0 4px 14px rgba(0,0,0,0.38) !important;
    opacity: 0.9 !important;
    display: inline-flex !important;
    align-items: center !important;
    justify-content: center !important;
    white-space: nowrap !important;
    transition: opacity 0.15s ease, transform 0.15s ease, box-shadow 0.15s ease !important;
  `);

  btn.addEventListener("mouseenter", () => {
    btn.style.setProperty("opacity", "1", "important");
    btn.style.setProperty("transform", "translateY(-1px) scale(1.02)", "important");
    btn.style.setProperty("box-shadow", "0 6px 18px rgba(0,0,0,0.48)", "important");
  });
  btn.addEventListener("mouseleave", () => {
    btn.style.setProperty("opacity", "0.9", "important");
    btn.style.setProperty("transform", "none", "important");
    btn.style.setProperty("box-shadow", "0 4px 14px rgba(0,0,0,0.38)", "important");
  });
  btn.addEventListener("pointerdown", (e) => {
    e.preventDefault();
    e.stopPropagation();
    e.stopImmediatePropagation();
  }, true);
  btn.addEventListener("pointerup", (e) => {
    e.preventDefault();
    e.stopPropagation();
    e.stopImmediatePropagation();
  }, true);
  btn.addEventListener("click", (e) => {
    e.preventDefault();
    e.stopPropagation();
    e.stopImmediatePropagation();
    onClick(btn);
  }, true);

  return btn;
}

function findTightVideoContainer(video) {
  const rect = video.getBoundingClientRect?.() ?? { width: 640, height: 360 };
  if (rect.width <= 0 || rect.height <= 0) {
    return video.parentElement;
  }
  let current = video.parentElement;
  let best = current;

  while (current && current !== document.body && current !== document.documentElement) {
    const cRect = current.getBoundingClientRect?.() ?? rect;
    // Do not climb into post cards, feeds, dialogs, or the whole page
    if (cRect.height > Math.max(rect.height * 1.25, rect.height + 60)) {
      break;
    }
    if (cRect.width > Math.max(rect.width * 1.25, rect.width + 100)) {
      break;
    }
    best = current;

    const style = window.getComputedStyle?.(current) ?? { position: "static" };
    if (
      (style.position === "relative" || style.position === "absolute") &&
      (current.matches?.('[data-testid*="video" i], [class*="video" i], [class*="player" i]') ||
       current.querySelector?.("video") === video)
    ) {
      if (cRect.width >= rect.width * 0.9 && cRect.height >= rect.height * 0.9) {
        best = current;
        break;
      }
    }
    current = current.parentElement;
  }

  return best || video.parentElement;
}

function findFacebookVideoContainer(video) {
  // Strategy 1: Known Facebook player wrappers
  const known = video.closest?.(
    'div[data-pagelet*="Video"], div[data-video-id], div[aria-label*="video" i], div[data-testid*="video" i], div[data-virtualized]'
  );
  if (known && !known.matches?.('div[role="article"], div[data-pagelet*="FeedUnit"], div[role="feed"], div[role="main"]')) {
    return known;
  }

  // Strategy 2: Climb up from video.parentElement, stopping before entering the post article card
  let cur = video.parentElement;
  let best = cur;

  while (cur && cur !== document.body && cur !== document.documentElement) {
    if (cur.matches?.('div[role="article"], div[data-pagelet*="FeedUnit"], div[role="feed"], div[role="main"]')) {
      break;
    }
    // If cur contains post action buttons or post caption text, we've climbed into the card content
    if (cur.querySelector?.('[data-ad-preview="message"], [aria-label*="Comment" i], [aria-label*="Like" i], [aria-label*="Share" i], form')) {
      break;
    }

    const r = cur.getBoundingClientRect?.();
    if (r && r.width >= 100 && r.height >= 60) {
      best = cur;
      const style = window.getComputedStyle?.(cur);
      if (style?.position === "relative" || style?.position === "absolute") {
        return cur;
      }
    }
    cur = cur.parentElement;
  }

  // If best is still the direct parent, climb one level if safe to clear inner overflow-hidden clipping
  if (best === video.parentElement && best?.parentElement) {
    const parent2 = best.parentElement;
    if (!parent2.matches?.('div[role="article"], div[data-pagelet*="FeedUnit"]') &&
        !parent2.querySelector?.('[data-ad-preview="message"], [aria-label*="Comment" i]')) {
      return parent2;
    }
  }

  return best || video.parentElement;
}

function extractFacebookVideoUrl(video, container) {
  if (/\/videos\/|\/reel\/|\/reels\/|\/watch/.test(window.location.pathname)) {
    return window.location.href;
  }

  const videoIdEl = video.closest?.("[data-video-id]") || container?.closest?.("[data-video-id]");
  if (videoIdEl) {
    const vid = videoIdEl.getAttribute("data-video-id");
    if (vid) return `https://www.facebook.com/watch/?v=${vid}`;
  }

  const post = video.closest?.('div[role="article"], div[data-pagelet*="FeedUnit"], article, div[data-virtualized]') || container?.parentElement;
  if (post) {
    const videoLink = post.querySelector?.(
      'a[href*="/videos/"], a[href*="/reel/"], a[href*="/reels/"], a[href*="/watch/"], a[href*="/watch?"], a[href*="fb.watch"], a[href*="video.php"]'
    );
    if (videoLink?.href) return videoLink.href;

    const permalink = post.querySelector?.(
      'a[href*="/posts/"], a[href*="permalink.php"], a[href*="story_fbid="]'
    );
    if (permalink?.href) return permalink.href;

    const timeLinks = post.querySelectorAll?.('span > a[role="link"], a[href*="facebook.com"]');
    for (const tl of timeLinks) {
      if (tl.href && !tl.href.includes("#") && !tl.href.includes("/groups/") && !tl.href.includes("/user/")) {
        if (/\/(posts|videos|reel|stories)\//.test(tl.href) || /story_fbid=/.test(tl.href)) {
          return tl.href;
        }
      }
    }
  }

  if (video.currentSrc && /^https?:\/\//.test(video.currentSrc) && !video.currentSrc.startsWith("blob:")) {
    return video.currentSrc;
  }
  if (video.src && /^https?:\/\//.test(video.src) && !video.src.startsWith("blob:")) {
    return video.src;
  }

  return window.location.href;
}

function extractFacebookVideoTitle(video, container) {
  const post = video.closest?.('div[role="article"], div[data-pagelet*="FeedUnit"], article, div[data-virtualized]') || container?.parentElement;
  if (post) {
    const textEl = post.querySelector?.('[data-ad-preview="message"], [dir="auto"]');
    if (textEl?.textContent?.trim()) {
      const t = textEl.textContent.trim();
      return t.length > 80 ? t.substring(0, 80) : t;
    }
  }
  return document.title || "Facebook Video";
}

function refreshInlineDownloadButtons() {
  const host = window.location.hostname;
  if (/youtube\.com|youtu\.be/.test(host)) {
    injectYouTubeButton();
  }
  if (/reddit\.com/.test(host)) {
    injectRedditButtons();
  }
  if (/x\.com|twitter\.com/.test(host)) {
    injectTwitterButtons();
  }
  if (/facebook\.com|fb\.watch/.test(host)) {
    injectFacebookButtons();
  }
  if (/instagram\.com/.test(host)) {
    injectInstagramButtons();
  }
  injectGenericVideoButtons();
}

function injectFacebookButtons() {
  const videos = document.querySelectorAll("video");
  for (const video of videos) {
    const rect = video.getBoundingClientRect?.() ?? { width: 0, height: 0 };
    const container = findFacebookVideoContainer(video);
    if (!container) continue;

    const cRect = container.getBoundingClientRect?.() ?? rect;
    const effectiveWidth = Math.max(rect.width, cRect.width);
    const effectiveHeight = Math.max(rect.height, cRect.height);
    if (effectiveWidth < 100 || effectiveHeight < 60) {
      continue;
    }

    if (container.querySelector(".ldm-site-btn")) {
      continue;
    }

    if (container.parentElement?.querySelector(":scope > .ldm-site-btn") ||
        video.parentElement?.querySelector(".ldm-site-btn")) {
      continue;
    }

    const btn = createSiteVideoBadge((buttonEl) => {
      const postUrl = extractFacebookVideoUrl(video, container) || window.location.href;
      const postTitle = extractFacebookVideoTitle(video, container);

      const candidate = {
        element: video,
        url: postUrl,
        kind: "media-fallback",
        title: postTitle
      };
      activeCandidate = candidate;
      triggerCaptureForCandidate(candidate, buttonEl);
    }, "Download");

    btn.style.setProperty("position", "absolute", "important");
    btn.style.setProperty("top", "12px", "important");
    btn.style.setProperty("right", "12px", "important");
    btn.style.setProperty("left", "auto", "important");
    btn.style.setProperty("z-index", "2147483647", "important");
    btn.style.setProperty("pointer-events", "auto", "important");

    const parentPos = window.getComputedStyle?.(container)?.position;
    if (!parentPos || parentPos === "static") {
      container.style.setProperty("position", "relative", "important");
    }

    container.appendChild(btn);
  }
}

function injectInstagramButtons() {
  for (const video of document.querySelectorAll("video")) {
    const rect = video.getBoundingClientRect?.() ?? { width: 640, height: 360 };
    const container = findTightVideoContainer(video);
    if (!container || container.querySelector(".ldm-site-btn")) continue;

    const cRect = container.getBoundingClientRect?.() ?? rect;
    if (Math.max(rect.width, cRect.width) < 120 || Math.max(rect.height, cRect.height) < 80) continue;

    const btn = createSiteVideoBadge((buttonEl) => {
      let postUrl = null;
      const post = video.closest?.("article, main, div[role='dialog']");
      const link = post?.querySelector?.('a[href*="/p/"], a[href*="/reel/"], a[href*="/reels/"]');
      if (link?.href) {
        postUrl = link.href;
      } else if (/\/p\/|\/reel\//.test(window.location.pathname)) {
        postUrl = window.location.href;
      }

      const candidate = {
        element: video,
        url: postUrl || window.location.href,
        kind: "media-fallback",
        title: "Instagram Video"
      };
      activeCandidate = candidate;
      triggerCaptureForCandidate(candidate, buttonEl);
    }, "Download");

    btn.style.setProperty("position", "absolute", "important");
    btn.style.setProperty("top", "12px", "important");
    btn.style.setProperty("right", "12px", "important");
    btn.style.setProperty("z-index", "2147483647", "important");

    const parentPos = window.getComputedStyle?.(container)?.position;
    if (!parentPos || parentPos === "static") {
      container.style.setProperty("position", "relative", "important");
    }
    container.appendChild(btn);
  }
}

function injectRedditButtons() {
  for (const player of document.querySelectorAll("shreddit-player, shreddit-player-2")) {
    if (player.querySelector?.(".ldm-site-btn")) continue;

    const post = player.closest?.("shreddit-post, article, [class*='Post']");
    if (!post) continue;
    if (post.querySelector?.(".ldm-site-btn")) continue;

    const btn = createSiteVideoBadge((buttonEl) => {
      let hlsUrl = null;
      const shadowRoot = player.shadowRoot;
      if (shadowRoot) {
        const videoEl = shadowRoot.querySelector?.("video");
        if (videoEl && videoEl.src && !videoEl.src.startsWith("blob:")) {
          hlsUrl = videoEl.src;
        }
      }

      if (!hlsUrl) {
        const src = player.getAttribute?.("src") || player.getAttribute?.("packaged-media-json");
        if (src) {
          try {
            const data = JSON.parse(src);
            hlsUrl = data?.playbackMp4s?.permutations?.[0]?.source?.url || data?.hlsUrl || data?.fallbackUrl;
          } catch (_) {
            if (src.includes(".m3u8") || src.includes("v.redd.it")) {
              hlsUrl = src;
            }
          }
        }
      }

      if (!hlsUrl) {
        const permalink = player.closest?.("shreddit-post")?.getAttribute?.("permalink");
        if (permalink) {
          hlsUrl = "https://www.reddit.com" + permalink;
        }
      }

      if (hlsUrl) {
        const shredditPost = player.closest?.("shreddit-post");
        const postTitle =
          shredditPost?.getAttribute?.("post-title") ||
          shredditPost?.querySelector?.('[slot="title"]')?.textContent?.trim() ||
          post.querySelector?.("h1, h3, [data-testid='post-title']")?.textContent?.trim() ||
          document.title;

        showToast("Sending download to Linux Download Manager...", "info");
        armCaptureTimeout();
        sendExtensionMessage({
          type: "capture-download",
          payload: {
            url: hlsUrl,
            sourcePageUrl: window.location.href,
            sourceTitle: postTitle
          }
        }, (response) => {
          if (chrome.runtime?.lastError) {
            clearCaptureTimeout();
            showToast(chrome.runtime.lastError.message, "error");
            return;
          }
          if (!response?.ok) {
            clearCaptureTimeout();
            showToast(response?.error ?? "Download failed.", "error");
          }
        });
      } else {
        showToast("Video URL not found.", "error");
      }
    }, "Download");

    player.insertAdjacentElement("beforebegin", btn);
  }
}

function injectTwitterButtons() {
  for (const video of document.querySelectorAll("video")) {
    const videoContainer = video.closest?.('[data-testid="videoComponent"], [data-testid="videoPlayer"]') || findTightVideoContainer(video);
    if (!videoContainer || videoContainer.querySelector(".ldm-site-btn")) continue;

    const rect = video.getBoundingClientRect?.() ?? { width: 640, height: 360 };
    const cRect = videoContainer.getBoundingClientRect?.() ?? rect;
    if (Math.max(rect.width, cRect.width) < 80 || Math.max(rect.height, cRect.height) < 60) continue;

    const tweet = video.closest?.("article");

    const btn = createSiteVideoBadge((buttonEl) => {
      let postUrl = null;
      const timeLink = tweet?.querySelector?.('a[href*="/status/"] time')?.closest?.("a");
      if (timeLink) postUrl = timeLink.href;
      else if (tweet?.querySelector?.('a[href*="/status/"]')) {
        postUrl = tweet.querySelector('a[href*="/status/"]').href;
      } else if (/\/status\/\d+/.test(window.location.pathname)) postUrl = window.location.href;

      const tweetText = tweet?.querySelector?.('[data-testid="tweetText"]')?.textContent?.trim();
      const userName = tweet?.querySelector?.('[data-testid="User-Name"] a')?.textContent?.trim();
      const title = tweetText
        ? (tweetText.length > 80 ? tweetText.substring(0, 80) : tweetText)
        : (userName ? `${userName} video` : null);

      const candidate = {
        element: video,
        url: postUrl || null,
        kind: "media-fallback",
        title: title
      };
      activeCandidate = candidate;
      triggerCaptureForCandidate(candidate, buttonEl);
    }, "Download");

    btn.style.setProperty("position", "absolute", "important");
    btn.style.setProperty("top", "12px", "important");
    btn.style.setProperty("right", "12px", "important");
    btn.style.setProperty("z-index", "2147483647", "important");

    const parentPos = window.getComputedStyle?.(videoContainer)?.position;
    if (!parentPos || parentPos === "static") {
      videoContainer.style.setProperty("position", "relative", "important");
    }
    videoContainer.appendChild(btn);
  }
}

function injectYouTubeButton() {
  const player = document.querySelector("#movie_player, .html5-video-player");
  if (player) {
    const existing = document.getElementById("ldm-yt-download-btn");
    if (!existing || !player.contains(existing)) {
      if (existing) existing.remove();

      const btn = createSiteVideoBadge((buttonEl) => {
        const videoEl = player.querySelector("video") || document.querySelector("video");
        const titleEl = document.querySelector("h1.ytd-watch-metadata, #title h1, h1.title, ytd-watch-metadata #title");
        const candidate = {
          element: videoEl || null,
          url: window.location.href,
          kind: "media-fallback",
          title: titleEl?.textContent?.trim() || document.title
        };
        activeCandidate = candidate;
        triggerCaptureForCandidate(candidate, buttonEl);
      }, "Download");

      btn.id = "ldm-yt-download-btn";
      btn.style.setProperty("top", "12px", "important");
      btn.style.setProperty("right", "12px", "important");
      btn.style.setProperty("left", "auto", "important");
      btn.style.setProperty("z-index", "2147483647", "important");

      const playerPos = window.getComputedStyle?.(player)?.position;
      if (playerPos === "static") {
        player.style.setProperty("position", "relative", "important");
      }
      player.appendChild(btn);
    }
  }

  if (window.location.pathname.startsWith("/shorts/")) {
    const activeShort = document.querySelector("ytd-reel-video-renderer[is-active], ytd-shorts [is-active]");
    if (activeShort && !activeShort.querySelector(".ldm-site-btn")) {
      const btn = createSiteVideoBadge((buttonEl) => {
        const candidate = {
          element: activeShort.querySelector("video") || null,
          url: window.location.href,
          kind: "media-fallback",
          title: activeShort.querySelector("#title, .title")?.textContent?.trim() || document.title
        };
        activeCandidate = candidate;
        triggerCaptureForCandidate(candidate, buttonEl);
      }, "Download");
      btn.style.setProperty("top", "20px", "important");
      btn.style.setProperty("right", "20px", "important");
      btn.style.setProperty("z-index", "2147483647", "important");
      const pos = window.getComputedStyle?.(activeShort)?.position;
      if (pos === "static") {
        activeShort.style.setProperty("position", "relative", "important");
      }
      activeShort.appendChild(btn);
    }
  }
}

function injectGenericVideoButtons() {
  if (isYtdlpSupportedSite()) return;

  for (const video of document.querySelectorAll("video")) {
    const rect = video.getBoundingClientRect?.() ?? { width: 640, height: 360 };
    if (rect.width < 140 || rect.height < 90) continue;

    const container = findTightVideoContainer(video);
    if (!container || container.querySelector?.(".ldm-site-btn")) continue;
    if (isCandidateAlreadyDecorated(container)) continue;

    const candidate = extractCandidateFromTarget(video) || {
      element: video,
      url: video.currentSrc || video.src || recentMediaCandidates[0] || null,
      kind: "media"
    };

    if (!candidate.url && recentMediaCandidates.length === 0) continue;

    const btn = createSiteVideoBadge((buttonEl) => {
      activeCandidate = candidate;
      triggerCaptureForCandidate(candidate, buttonEl);
    }, "Download");

    const parentPos = window.getComputedStyle?.(container)?.position;
    if (parentPos === "static") {
      container.style.setProperty("position", "relative", "important");
    }
    container.appendChild(btn);
  }
}

function isSameCandidate(left, right) {
  return left?.url === right?.url && left?.kind === right?.kind;
}

function collectOverlayTargets() {
  const results = [];
  const seen = new Set();

  for (const container of document.querySelectorAll(MEDIA_CONTAINER_SELECTOR)) {
    if (!(container instanceof Element)) {
      continue;
    }

    const candidate = extractCandidateFromMediaContainer(container);
    if (!candidate) {
      continue;
    }

    if (seen.has(candidate.element)) {
      continue;
    }

    seen.add(candidate.element);
    results.push({ element: candidate.element, candidate });
  }

  for (const mediaElement of document.querySelectorAll("video, audio")) {
    if (!(mediaElement instanceof HTMLMediaElement)) {
      continue;
    }

    const candidate = extractCandidateFromTarget(mediaElement);
    if (!candidate) {
      continue;
    }

    const overlayElement = resolveOverlayElement(mediaElement);
    if (!overlayElement) {
      continue;
    }

    if (seen.has(overlayElement)) {
      continue;
    }
    seen.add(overlayElement);
    results.push({ element: overlayElement, candidate });
  }

  return results;
}

function extractCandidateFromMediaContainer(container) {
  if (!(container instanceof Element)) {
    return null;
  }

  if (!looksLikeMediaSurface(container)) {
    return null;
  }

  const mediaElement = container.querySelector("video, audio, source");
  if (mediaElement instanceof Element) {
    const mediaCandidate = extractCandidateFromTarget(mediaElement);
    if (mediaCandidate) {
      return {
        ...mediaCandidate,
        element: resolveOverlayElement(
          mediaElement.tagName === "SOURCE" && mediaElement.parentElement
            ? mediaElement.parentElement
            : mediaElement
        )
      };
    }

    return {
      element: container,
      url: recentMediaCandidates[0] ?? null,
      kind: "media-fallback"
    };
  }

  const url = resolveMediaContainerUrl(container);
  if (url && isMediaCandidate(url)) {
    return {
      element: container,
      url,
      kind: "media"
    };
  }

  if (recentMediaCandidates.length > 0) {
    return {
      element: container,
      url: recentMediaCandidates[0],
      kind: "media-fallback"
    };
  }

  return null;
}

function resolveActiveOverlayTarget() {
  if (!activeCandidate) {
    return null;
  }

  const overlayElement = resolveCandidateAnchorElement(activeCandidate);
  if (!(overlayElement instanceof Element)) {
    return null;
  }

  return {
    element: overlayElement,
    candidate: activeCandidate
  };
}

function resolveCandidateAnchorElement(candidate) {
  if (!candidate?.element) {
    return null;
  }

  if (candidate.element instanceof HTMLMediaElement) {
    return resolveOverlayElement(candidate.element);
  }

  if (!(candidate.element instanceof Element)) {
    return null;
  }

  const nestedMedia = candidate.element.querySelector("video, audio");
  if (nestedMedia instanceof HTMLMediaElement) {
    return resolveOverlayElement(nestedMedia);
  }

  const mediaContainer = findMediaContainer(candidate.element);
  if (mediaContainer && looksLikeMediaSurface(mediaContainer)) {
    return mediaContainer;
  }

  if (looksLikeMediaSurface(candidate.element)) {
    return candidate.element;
  }

  return null;
}

function resolveOverlayElement(mediaElement) {
  const overlayElement = resolveGenericOverlayElement(mediaElement);
  if (overlayElement) {
    return overlayElement;
  }

  return mediaElement;
}

function resolveGenericOverlayElement(mediaElement, preferredContainer = null) {
  const mediaRect = mediaElement.getBoundingClientRect();
  const containers = collectOverlayContainers(mediaElement, preferredContainer);

  let bestContainer = null;
  let bestScore = Number.POSITIVE_INFINITY;

  for (const container of containers) {
    const rect = container.getBoundingClientRect();
    if (!isVisibleMediaRect(rect)) {
      continue;
    }

    if (!rectContains(rect, mediaRect)) {
      continue;
    }

    const areaRatio = (rect.width * rect.height) / Math.max(1, mediaRect.width * mediaRect.height);
    const offset =
      Math.abs(rect.left - mediaRect.left) +
      Math.abs(rect.top - mediaRect.top) +
      Math.abs(rect.right - mediaRect.right) +
      Math.abs(rect.bottom - mediaRect.bottom);
    const score = Math.abs(areaRatio - 1) * 100 + offset;
    if (score < bestScore) {
      bestScore = score;
      bestContainer = container;
    }
  }

  return bestContainer;
}

function collectOverlayContainers(mediaElement, preferredContainer = null) {
  const containers = [];
  const seen = new Set();

  const addContainer = (element) => {
    if (!(element instanceof Element) || seen.has(element)) {
      return;
    }

    seen.add(element);
    containers.push(element);
  };

  addContainer(preferredContainer);
  addContainer(mediaElement.parentElement);
  addContainer(mediaElement.closest("figure"));
  addContainer(mediaElement.closest("picture"));
  addContainer(mediaElement.closest('[data-testid*="video" i]'));
  addContainer(mediaElement.closest('[data-testid*="media" i]'));
  addContainer(mediaElement.closest('[class*="video" i]'));
  addContainer(mediaElement.closest('[class*="player" i]'));
  addContainer(mediaElement.closest('[class*="media" i]'));
  addContainer(mediaElement.closest('[role="dialog"]'));
  addContainer(mediaElement.closest('[role="button"]'));
  addContainer(mediaElement.closest('[role="link"]'));
  addContainer(mediaElement.closest("a[href]"));
  addContainer(mediaElement.closest("article"));
  addContainer(mediaElement.closest("section"));

  let ancestor = mediaElement.parentElement;
  let depth = 0;
  while (ancestor && depth < 7) {
    addContainer(ancestor);
    ancestor = ancestor.parentElement;
    depth += 1;
  }

  return containers;
}

function showToast(message, tone = "info") {
  if (!toastRoot) {
    return;
  }

  if (toastTimer) {
    window.clearTimeout(toastTimer);
  }

  const toast = document.createElement("div");
  toast.className = `ldm-toast ldm-toast-${tone}`;
  toast.textContent = message;
  toastRoot.replaceChildren(toast);

  toastTimer = window.setTimeout(() => {
    toastRoot?.replaceChildren();
  }, 4500);
}

function findMediaContainer(target) {
  if (!(target instanceof Element)) {
    return null;
  }

  const container = target.closest(MEDIA_CONTAINER_SELECTOR);
  return container instanceof Element ? container : null;
}

function armCaptureTimeout() {
  clearCaptureTimeout();
  captureTimeout = window.setTimeout(() => {
    showToast("No response from extension background. Reload the extension and try again.", "error");
  }, 2500);
}

function clearCaptureTimeout() {
  if (captureTimeout) {
    window.clearTimeout(captureTimeout);
    captureTimeout = null;
  }
}

function isVisibleMediaRect(rect) {
  if (rect.width < 80 || rect.height < 60) {
    return false;
  }

  if (rect.bottom < 0 || rect.right < 0) {
    return false;
  }

  if (rect.top > window.innerHeight || rect.left > window.innerWidth) {
    return false;
  }

  return true;
}

function rectContains(outer, inner) {
  return (
    outer.left <= inner.left + 4 &&
    outer.top <= inner.top + 4 &&
    outer.right >= inner.right - 4 &&
    outer.bottom >= inner.bottom - 4
  );
}

function findVisibleMediaAtPoint(clientX, clientY) {
  const matches = [];
  for (const mediaElement of document.querySelectorAll("video, audio")) {
    if (!(mediaElement instanceof HTMLMediaElement)) {
      continue;
    }

    const rect = mediaElement.getBoundingClientRect();
    if (rect.width < 40 || rect.height < 40) {
      continue;
    }

    const insideX = clientX >= rect.left && clientX <= rect.right;
    const insideY = clientY >= rect.top && clientY <= rect.bottom;
    if (insideX && insideY) {
      matches.push(mediaElement);
    }
  }

  return matches;
}

function normalizeUrl(rawUrl) {
  if (!rawUrl) {
    return null;
  }

  try {
    const url = new URL(rawUrl, window.location.href);
    if (!/^https?:$/i.test(url.protocol)) {
      return null;
    }

    return url.toString();
  } catch (error) {
    return null;
  }
}

function looksLikeMediaSurface(element) {
  if (!(element instanceof Element)) {
    return false;
  }

  const rect = element.getBoundingClientRect();
  if (!isVisibleMediaRect(rect)) {
    return false;
  }

  if (rect.width < 180 || rect.height < 120) {
    return false;
  }

  const hasMediaChildren = Boolean(element.querySelector("video, audio, source, img, canvas"));
  const hasPlayableRole =
    element.matches('[role="button"], [role="link"], a[href]') ||
    element.querySelector('[aria-label*="play" i], [data-testid*="play" i], button');
  return hasMediaChildren || Boolean(hasPlayableRole);
}

function resolveMediaContainerUrl(container) {
  if (!(container instanceof Element)) {
    return null;
  }

  for (const attribute of MEDIA_SOURCE_ATTRIBUTES) {
    const url = normalizeUrl(container.getAttribute(attribute));
    if (url) {
      return url;
    }
  }

  for (const node of container.querySelectorAll("[src], [data-src], [data-url], [data-video-src], [data-video-url], [data-stream], [data-stream-url], [data-hls], [data-mpd]")) {
    if (!(node instanceof Element)) {
      continue;
    }

    for (const attribute of MEDIA_SOURCE_ATTRIBUTES) {
      const url = normalizeUrl(node.getAttribute(attribute));
      if (url) {
        return url;
      }
    }
  }

  return null;
}

function resolveMediaUrl(mediaTarget, mediaElement) {
  const directCandidates = [
    mediaTarget instanceof HTMLMediaElement ? mediaTarget.currentSrc : null,
    mediaElement instanceof HTMLMediaElement ? mediaElement.currentSrc : null
  ];

  for (const candidate of directCandidates) {
    const url = normalizeUrl(candidate);
    if (url) {
      return url;
    }
  }

  const nodesToInspect = [mediaTarget, mediaElement];
  if (mediaElement instanceof Element) {
    nodesToInspect.push(...mediaElement.querySelectorAll("source, [src], [data-src], [data-url]"));
  }

  for (const node of nodesToInspect) {
    if (!(node instanceof Element)) {
      continue;
    }

    for (const attribute of MEDIA_SOURCE_ATTRIBUTES) {
      const url = normalizeUrl(node.getAttribute(attribute));
      if (url) {
        return url;
      }
    }
  }

  for (const candidate of recentMediaCandidates) {
    if (isMediaCandidate(candidate.url)) {
      return candidate.url;
    }
  }

  return null;
}

function resolvePersistentMediaCandidate() {
  for (const mediaElement of document.querySelectorAll("video, audio")) {
    if (!(mediaElement instanceof HTMLMediaElement)) {
      continue;
    }

    if (!isVisibleMediaRect(mediaElement.getBoundingClientRect())) {
      continue;
    }

    if (!mediaElement.paused || mediaElement.readyState > 1) {
      const candidate = extractCandidateFromTarget(mediaElement);
      if (candidate) {
        return candidate;
      }
    }
  }

  for (const mediaElement of document.querySelectorAll("video, audio")) {
    if (!(mediaElement instanceof HTMLMediaElement)) {
      continue;
    }

    if (!isVisibleMediaRect(mediaElement.getBoundingClientRect())) {
      continue;
    }

    const candidate = extractCandidateFromTarget(mediaElement);
    if (candidate) {
      return candidate;
    }
  }

  for (const container of document.querySelectorAll(MEDIA_CONTAINER_SELECTOR)) {
    if (!(container instanceof Element)) {
      continue;
    }

    const candidate = extractCandidateFromMediaContainer(container);
    if (candidate) {
      return candidate;
    }
  }

  return null;
}

function isDownloadableLink(link, url) {
  const normalized = normalizeUrl(url);
  if (!normalized) return false;
  if (link.hasAttribute("download")) {
    return true;
  }

  const parsedUrl = new URL(normalized);
  if (parsedUrl.searchParams.has("download")) {
    return true;
  }

  const extension = parsedUrl.pathname.toLowerCase().split(".").pop();
  return DOWNLOADABLE_EXTENSIONS.has(extension);
}

function isMediaCandidate(url) {
  const normalized = normalizeUrl(url);
  if (!normalized) return false;
  const parsedUrl = new URL(normalized);
  const extension = parsedUrl.pathname.toLowerCase().split(".").pop();
  return DOWNLOADABLE_EXTENSIONS.has(extension) || ["m3u8", "mpd", "m4s", "ts", "aac"].includes(extension);
}

function refreshRecentMediaCandidates() {
  sendExtensionMessage({ type: "get-media-candidates" }, (response) => {
    if (chrome.runtime.lastError) {
      return;
    }

    if (!response?.ok || !Array.isArray(response.candidates)) {
      return;
    }

    recentMediaCandidates = response.candidates
      .map((candidate) => normalizeUrl(candidate?.url))
      .filter(Boolean);
  });
}

function reportObservedMediaCandidates() {
  if (extensionDisconnected) return;
  sendExtensionMessage({type: "capture-diagnostic-frame",
    media: Array.from(document.querySelectorAll("video, audio")).map(m => ({
      src: m.currentSrc || m.src, readyState: m.readyState, paused: m.paused})),
    embeds: Array.from(document.querySelectorAll("iframe")).map(f => f.src),
    resourceCount: performance.getEntriesByType("resource").length
  }, () => void chrome.runtime.lastError);
  const candidates = [];
  const seen = new Set();
  const referrerUrl = window.location.href;
  const observedHeaders = {
    "referer": referrerUrl,
    "user-agent": navigator.userAgent,
    "accept": "*/*"
  };
  if (window.location.origin && window.location.origin !== "null") {
    observedHeaders.origin = window.location.origin;
  }

  const remember = (url, type = "observed") => {
    const normalized = normalizeUrl(url);
    if (!normalized || seen.has(normalized) || (!["media", "audio"].includes(type) && !isLikelyObservedMediaUrl(normalized))) {
      return;
    }

    seen.add(normalized);
    candidates.push({
      url: normalized,
      type,
      referrerUrl,
      httpHeaders: observedHeaders
    });
  };

  for (const mediaElement of document.querySelectorAll("video, audio")) {
    if (!(mediaElement instanceof HTMLMediaElement)) {
      continue;
    }

    const mediaType = mediaElement.tagName === "AUDIO" ? "audio" : "media";
    remember(mediaElement.currentSrc, mediaType);
    remember(mediaElement.src, mediaType);

    for (const source of mediaElement.querySelectorAll("source")) {
      if (!(source instanceof HTMLSourceElement)) {
        continue;
      }

      remember(source.src, mediaType);
      remember(source.getAttribute("src"), mediaType);
    }
  }

  for (const entry of performance.getEntriesByType("resource")) {
    if (!entry || typeof entry.name !== "string") {
      continue;
    }

    const initiatorType = typeof entry.initiatorType === "string" ? entry.initiatorType : "observed";
    if (["video", "audio", "xmlhttprequest", "fetch", "other"].includes(initiatorType) || isLikelyObservedMediaUrl(entry.name)) {
      remember(entry.name, initiatorType);
    }
  }

  for (const url of playerManifests) {
    candidates.unshift({url, type: "observed-manifest", referrerUrl, httpHeaders: observedHeaders});
  }
  if (candidates.length === 0) return;

  sendExtensionMessage({
    type: "remember-media-candidates",
    candidates: candidates.slice(0, 24)
  }, () => void chrome.runtime.lastError);
}

function isLikelyObservedMediaUrl(url) {
  try {
    const parsedUrl = new URL(url);
    if (!/^https?:$/i.test(parsedUrl.protocol)) {
      return false;
    }

    const pathname = parsedUrl.pathname.toLowerCase();
    const extension = pathname.split(".").pop();
    if (IMAGE_EXTENSIONS.has(extension)) {
      return false;
    }
    if (DOWNLOADABLE_EXTENSIONS.has(extension) || ["m3u8", "mpd", "m4s", "ts", "aac", "m3u"].includes(extension)) {
      return true;
    }

    const mimeType =
      parsedUrl.searchParams.get("mime_type") ||
      parsedUrl.searchParams.get("mime") ||
      parsedUrl.searchParams.get("content_type") ||
      "";
    if (/video|audio/i.test(mimeType)) {
      return true;
    }

    if (/\/(videoplayback|manifest|playlist|master|hls|dash)\b/i.test(pathname)) {
      return true;
    }

    if (/(^|\.)fbcdn\.net$/i.test(parsedUrl.hostname) || /(^|\.)cdninstagram\.com$/i.test(parsedUrl.hostname)) {
      return true;
    }

    if (parsedUrl.searchParams.has("bytestart") || parsedUrl.searchParams.has("byteend")) {
      return true;
    }

    const formatHint =
      parsedUrl.searchParams.get("format") ||
      parsedUrl.searchParams.get("ext") ||
      parsedUrl.searchParams.get("filename") ||
      "";
    return /\.(mp4|webm|mov|mkv|mp3|m4a|m3u8|mpd)\b/i.test(formatHint);
  } catch (error) {
    return false;
  }
}
