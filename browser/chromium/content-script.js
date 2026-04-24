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
let lastLocationHref = window.location.href;
let periodicRefreshHandle = null;

bootstrap();

function isExcludedHost(hostname) {
  return /(^|\.)(whatsapp\.com|whatsapp\.net)$/i.test(hostname);
}

function bootstrap() {
  if (isExcludedHost(window.location.hostname)) {
    return;
  }
  createHoverButton();
  createMediaOverlay();
  createToastRoot();
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
  if (hoverButton) {
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

function hasInlineButtons() {
  return /reddit\.com|youtube\.com|x\.com|twitter\.com|facebook\.com|instagram\.com/i.test(window.location.hostname);
}

function handlePointerOver(event) {
  if (hasInlineButtons()) return;

  const candidate = extractCandidateFromPoint(event);
  if (!candidate) {
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
  return /(youtube\.com|youtu\.be|x\.com|twitter\.com|facebook\.com|instagram\.com|fb\.watch)$/i.test(host);
}

function showQualityPicker(candidate, anchorElement) {
  closeQualityPicker();

  const qualities = [
    { label: "En iyi kalite", format: "bv*+ba/b" },
    { label: "4K (2160p)", format: "bv*[height<=2160]+ba/b" },
    { label: "1440p", format: "bv*[height<=1440]+ba/b" },
    { label: "1080p", format: "bv*[height<=1080]+ba/b" },
    { label: "720p", format: "bv*[height<=720]+ba/b" },
    { label: "480p", format: "bv*[height<=480]+ba/b" },
    { label: "360p", format: "bv*[height<=360]+ba/b" },
    { label: "Sadece ses", format: "ba/b" }
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
  title.textContent = "Kalite Seçin";
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
  if (candidate?.url && /^https?:\/\/.*(reddit\.com|x\.com|twitter\.com|youtube\.com|facebook\.com|instagram\.com)\//.test(candidate.url)) {
    return candidate.url;
  }
  return window.location.href;
}

function triggerCaptureWithFormat(candidate, format) {
  showToast("Sending download to Linux Download Manager...", "info");
  armCaptureTimeout();

  const sourcePageUrl = resolveSourcePageUrl(candidate);

  if (candidate.kind === "media" || candidate.kind === "media-fallback") {
    chrome.runtime.sendMessage({
      type: "capture-best-media",
      payload: {
        preferredUrl: candidate.url ?? null,
        sourcePageUrl: sourcePageUrl,
        sourceTitle: candidate.title || document.title,
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
        showToast(response?.error ?? "Media capture failed.", "error");
        return;
      }
      pulseHoverButton();
    });
    return;
  }

  chrome.runtime.sendMessage({
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
  if (!hoverButton || !activeCandidate) {
    hideHoverButton();
    return;
  }

  if (isStickyMediaCandidate(activeCandidate)) {
    const stickyElement = resolveCandidateAnchorElement(activeCandidate);
    if (stickyElement) {
      const rect = stickyElement.getBoundingClientRect();
      if (!isVisibleMediaRect(rect)) {
        hideHoverButton();
        return;
      }

      hoverButton.style.position = "absolute";
      hoverButton.style.right = "auto";
      hoverButton.style.top = `${Math.max(8, rect.top + window.scrollY + 10)}px`;
      hoverButton.style.left = `${Math.max(8, rect.right + window.scrollX - 44)}px`;
      hoverButton.hidden = false;
      return;
    }

    hoverButton.style.position = "fixed";
    hoverButton.style.top = "18px";
    hoverButton.style.left = "auto";
    hoverButton.style.right = "18px";
    hoverButton.hidden = false;
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
  const observer = new MutationObserver(() => {
    detectLocationChange();
    scheduleCandidateReport();
    scheduleMediaRefresh();
    scheduleMediaOverlayRefresh();
    if (activeCandidate && !activeCandidate.element.isConnected && !isStickyMediaCandidate(activeCandidate)) {
      hideHoverButton();
    } else if (isStickyMediaCandidate(activeCandidate)) {
      const recoveredCandidate = resolvePersistentMediaCandidate();
      if (recoveredCandidate) {
        activeCandidate = recoveredCandidate;
        repositionHoverButton();
      }
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
  if (event?.target instanceof Element) {
    const candidate = extractCandidateFromTarget(event.target);
    if (candidate) {
      activeCandidate = candidate;
      repositionHoverButton();
    }
  }

  scheduleCandidateReport();
  scheduleMediaRefresh();
  scheduleMediaOverlayRefresh();
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
  scheduleCandidateReport();
  scheduleMediaRefresh();
  scheduleMediaOverlayRefresh();
}

function startPeriodicRefresh() {
  if (periodicRefreshHandle) {
    window.clearInterval(periodicRefreshHandle);
  }

  periodicRefreshHandle = window.setInterval(() => {
    detectLocationChange();
    scheduleMediaRefresh();
    scheduleMediaOverlayRefresh();
  }, 1200);
}

function scheduleMediaRefresh() {
  window.setTimeout(() => {
    reportObservedMediaCandidates();
    refreshRecentMediaCandidates();
  }, 180);
}

function scheduleMediaOverlayRefresh() {
  if (mediaRefreshTimer) {
    window.clearTimeout(mediaRefreshTimer);
  }

  mediaRefreshTimer = window.setTimeout(() => {
    mediaRefreshTimer = null;
    refreshMediaOverlay();
  }, 120);
}

function scheduleCandidateReport() {
  if (reportTimer) {
    window.clearTimeout(reportTimer);
  }

  reportTimer = window.setTimeout(() => {
    reportTimer = null;
    chrome.runtime.sendMessage({
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

  mediaOverlayRoot.replaceChildren();

  refreshInlineDownloadButtons();

  const host = window.location.hostname;
  if (/reddit\.com|youtube\.com|x\.com|twitter\.com|facebook\.com|instagram\.com/.test(host)) {
    return;
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
            pinned: true
          }
    );
  }

  for (const target of overlayTargets) {
    if (target.element?.querySelector?.(".ldm-inline-btn")) {
      continue;
    }

    const button = document.createElement("button");
    button.type = "button";
    button.className = "ldm-media-button";
    button.textContent = "Download";
    if (target.pinned) {
      button.classList.add("ldm-media-button-floating");
      button.style.top = "18px";
      button.style.right = "86px";
      button.style.left = "auto";
    } else {
      const rect = target.element.getBoundingClientRect();
      if (!isVisibleMediaRect(rect)) {
        continue;
      }

      button.style.top = `${Math.max(12, rect.top + 12)}px`;
      button.style.left = `${Math.max(12, rect.right - 108)}px`;
      button.style.right = "auto";
    }

    button.addEventListener("pointerdown", (event) => {
      event.preventDefault();
      event.stopPropagation();
    });
    button.addEventListener("click", (event) => {
      event.preventDefault();
      event.stopPropagation();
      activeCandidate = target.candidate;
      triggerCaptureForCandidate(target.candidate, button);
    });
    mediaOverlayRoot.appendChild(button);
  }
}

function refreshInlineDownloadButtons() {
  const host = window.location.hostname;
  if (/youtube\.com/.test(host)) {
    injectYouTubeButton();
  }
  if (/reddit\.com/.test(host)) {
    injectRedditButtons();
  }
  if (/x\.com|twitter\.com/.test(host)) {
    injectTwitterButtons();
  }
}

function injectRedditButtons() {
  for (const player of document.querySelectorAll("shreddit-player, shreddit-player-2")) {
    if (player.querySelector(".ldm-site-btn")) continue;

    const post = player.closest("shreddit-post, article, [class*='Post']");
    if (!post) continue;
    if (post.querySelector(".ldm-site-btn")) continue;

    const btn = document.createElement("div");
    btn.className = "ldm-site-btn";
    btn.textContent = "⬇ LDM İndir";
    btn.setAttribute("style", `
      display: block !important;
      z-index: 2147483647 !important;
      background: linear-gradient(135deg, #3dd29f, #86e8ff) !important;
      color: #04110d !important;
      border: 0 !important;
      border-radius: 6px !important;
      padding: 8px 16px !important;
      font: 700 12px/1 sans-serif !important;
      letter-spacing: 0.04em !important;
      cursor: pointer !important;
      pointer-events: auto !important;
      user-select: none !important;
      box-shadow: 0 2px 8px rgba(0,0,0,0.15) !important;
      margin: 6px 0 !important;
      width: fit-content !important;
    `);

    btn.addEventListener("mouseenter", () => btn.style.setProperty("opacity", "1", "important"));
    btn.addEventListener("mouseleave", () => btn.style.setProperty("opacity", "0.85", "important"));
    btn.addEventListener("pointerdown", (e) => { e.preventDefault(); e.stopPropagation(); e.stopImmediatePropagation(); }, true);
    btn.addEventListener("click", (e) => {
      e.preventDefault();
      e.stopPropagation();
      e.stopImmediatePropagation();

      let hlsUrl = null;
      const shadowRoot = player.shadowRoot;
      if (shadowRoot) {
        const videoEl = shadowRoot.querySelector("video");
        if (videoEl && videoEl.src && !videoEl.src.startsWith("blob:")) {
          hlsUrl = videoEl.src;
        }
      }

      if (!hlsUrl) {
        const src = player.getAttribute("src")
          || player.getAttribute("packaged-media-json");
        if (src) {
          try {
            const data = JSON.parse(src);
            hlsUrl = data?.playbackMp4s?.permutations?.[0]?.source?.url
              || data?.hlsUrl
              || data?.fallbackUrl;
          } catch (_) {
            if (src.includes(".m3u8") || src.includes("v.redd.it")) {
              hlsUrl = src;
            }
          }
        }
      }

      if (!hlsUrl) {
        const permalink = player.closest("shreddit-post")?.getAttribute("permalink");
        if (permalink) {
          hlsUrl = "https://www.reddit.com" + permalink;
        }
      }

      if (hlsUrl) {
        const shredditPost = player.closest("shreddit-post");
        const postTitle = shredditPost?.getAttribute("post-title")
          || shredditPost?.querySelector('[slot="title"]')?.textContent?.trim()
          || post.querySelector("h1, h3, [data-testid='post-title']")?.textContent?.trim()
          || document.title;

        showToast("Sending download to Linux Download Manager...", "info");
        armCaptureTimeout();
        chrome.runtime.sendMessage({
          type: "capture-download",
          payload: {
            url: hlsUrl,
            sourcePageUrl: window.location.href,
            sourceTitle: postTitle
          }
        }, (response) => {
          if (chrome.runtime.lastError) {
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
        showToast("Video URL bulunamadı.", "error");
      }
    }, true);

    player.insertAdjacentElement("beforebegin", btn);
  }
}

function injectTwitterButtons() {
  const tweets = document.querySelectorAll("article");
  for (const tweet of tweets) {
    if (tweet.querySelector(".ldm-site-btn")) continue;

    const video = tweet.querySelector("video");
    if (!video || !(video instanceof HTMLMediaElement)) continue;

    const videoContainer = video.closest('[data-testid="videoComponent"], [data-testid="videoPlayer"]') || video.parentElement;
    if (tweet.closest("[data-testid]")?.querySelector(".ldm-site-btn")) continue;

    const rect = video.getBoundingClientRect();
    if (rect.width < 80 || rect.height < 60) continue;

    let postUrl = null;
    const timeLink = tweet.querySelector('a[href*="/status/"] time')?.closest("a");
    if (timeLink) postUrl = timeLink.href;

    const btn = document.createElement("div");
    btn.className = "ldm-site-btn";
    btn.textContent = "⬇ LDM";
    btn.setAttribute("style", `
      position: absolute !important;
      top: 8px !important;
      right: 8px !important;
      z-index: 2147483647 !important;
      background: linear-gradient(135deg, #3dd29f, #86e8ff) !important;
      color: #04110d !important;
      border: 0 !important;
      border-radius: 6px !important;
      padding: 6px 14px !important;
      font: 700 12px/1 sans-serif !important;
      cursor: pointer !important;
      pointer-events: auto !important;
      user-select: none !important;
      box-shadow: 0 4px 12px rgba(0,0,0,0.4) !important;
      opacity: 0.85 !important;
      transition: opacity 0.15s !important;
    `);

    btn.addEventListener("mouseenter", () => btn.style.setProperty("opacity", "1", "important"));
    btn.addEventListener("mouseleave", () => btn.style.setProperty("opacity", "0.85", "important"));
    btn.addEventListener("pointerdown", (e) => { e.preventDefault(); e.stopPropagation(); e.stopImmediatePropagation(); }, true);
    btn.addEventListener("click", (e) => {
      e.preventDefault();
      e.stopPropagation();
      e.stopImmediatePropagation();

      const tweetText = tweet.querySelector('[data-testid="tweetText"]')?.textContent?.trim();
      const userName = tweet.querySelector('[data-testid="User-Name"] a')?.textContent?.trim();
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
      triggerCaptureForCandidate(candidate, btn);
    }, true);

    const parentPos = window.getComputedStyle(videoContainer).position;
    if (parentPos === "static") {
      videoContainer.style.setProperty("position", "relative", "");
    }
    videoContainer.appendChild(btn);
  }
}

function injectYouTubeButton() {
  if (document.getElementById("ldm-yt-download-btn")) return;

  const player = document.querySelector("#movie_player, .html5-video-player");
  if (!player) return;

  const btn = document.createElement("div");
  btn.id = "ldm-yt-download-btn";
  btn.textContent = "LDM";
  btn.setAttribute("style", `
    position: absolute !important;
    top: 12px !important;
    right: 12px !important;
    z-index: 2147483647 !important;
    background: linear-gradient(135deg, #3dd29f, #86e8ff) !important;
    color: #04110d !important;
    border: 0 !important;
    border-radius: 6px !important;
    padding: 8px 16px !important;
    font: 700 13px/1 sans-serif !important;
    letter-spacing: 0.08em !important;
    cursor: pointer !important;
    text-transform: uppercase !important;
    opacity: 0.85 !important;
    pointer-events: auto !important;
    user-select: none !important;
    transition: opacity 0.15s !important;
    box-shadow: 0 4px 16px rgba(0,0,0,0.4) !important;
  `);

  btn.addEventListener("mouseenter", () => {
    btn.style.setProperty("opacity", "1", "important");
  });
  btn.addEventListener("mouseleave", () => {
    btn.style.setProperty("opacity", "0.85", "important");
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

    const videoEl = document.querySelector("#movie_player video");
    const candidate = {
      element: videoEl || null,
      url: null,
      kind: "media-fallback"
    };
    activeCandidate = candidate;
    triggerCaptureForCandidate(candidate, btn);
  }, true);

  player.style.setProperty("position", "relative", "");
  player.appendChild(btn);
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
  if (link.hasAttribute("download")) {
    return true;
  }

  const parsedUrl = new URL(url);
  if (parsedUrl.searchParams.has("download")) {
    return true;
  }

  const extension = parsedUrl.pathname.toLowerCase().split(".").pop();
  return DOWNLOADABLE_EXTENSIONS.has(extension);
}

function isMediaCandidate(url) {
  const parsedUrl = new URL(url);
  const extension = parsedUrl.pathname.toLowerCase().split(".").pop();
  return DOWNLOADABLE_EXTENSIONS.has(extension) || ["m3u8", "mpd", "m4s", "ts", "aac"].includes(extension);
}

function refreshRecentMediaCandidates() {
  chrome.runtime.sendMessage({ type: "get-media-candidates" }, (response) => {
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
  const candidates = [];
  const seen = new Set();

  const remember = (url, type = "observed") => {
    const normalized = normalizeUrl(url);
    if (!normalized || seen.has(normalized) || !isLikelyObservedMediaUrl(normalized)) {
      return;
    }

    seen.add(normalized);
    candidates.push({ url: normalized, type });
  };

  for (const mediaElement of document.querySelectorAll("video, audio")) {
    if (!(mediaElement instanceof HTMLMediaElement)) {
      continue;
    }

    remember(mediaElement.currentSrc, "observed");
    remember(mediaElement.src, "observed");

    for (const source of mediaElement.querySelectorAll("source")) {
      if (!(source instanceof HTMLSourceElement)) {
        continue;
      }

      remember(source.src, "observed");
      remember(source.getAttribute("src"), "observed");
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

  if (candidates.length === 0) {
    return;
  }

  chrome.runtime.sendMessage({
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

    if (/\/(videoplayback|manifest|playlist|master|video|hls|dash)\b/i.test(pathname)) {
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
