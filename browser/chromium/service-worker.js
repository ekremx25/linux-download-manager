const HOST_NAME = "com.eko.linuxdownloadmanager";
const MENU_DOWNLOAD_LINK = "linux-download-manager.link";
const MENU_DOWNLOAD_PAGE = "linux-download-manager.page";
const MENU_DOWNLOAD_MEDIA = "linux-download-manager.media";
const BADGE_COLOR = "#0e9f6e";
const MAX_MEDIA_CANDIDATES = 12;
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
const STREAMING_EXTENSIONS = new Set(["m3u8", "mpd", "m4s", "ts", "aac", "m3u"]);
const IMAGE_EXTENSIONS = new Set(["avif", "bmp", "gif", "jpeg", "jpg", "png", "svg", "webp"]);
const recentCaptures = new Map();
const recentMediaByTab = new Map();
const captureDiagnostics = new Map();
function diagnosticUrl(raw) {
  try { const u = new URL(raw); return { origin: u.origin, protocol: u.protocol,
    extension: u.pathname.match(/\.([a-z0-9]{1,8})$/i)?.[1] ?? "none" }; }
  catch { return { protocol: "unknown" }; }
}
function diagnosticState(tabId) {
  if (!captureDiagnostics.has(tabId)) captureDiagnostics.set(tabId, { frames: {}, requests: [] });
  return captureDiagnostics.get(tabId);
}
function saveCaptureDiagnostic(tabId, page) {
  const state = diagnosticState(tabId);
  const report = {version: chrome.runtime.getManifest().version,
    page: diagnosticUrl(page), ...state,
    candidates: getMediaCandidatesForTab(tabId).map(c => ({...diagnosticUrl(c.url), kind: c.streamKind, type: c.type}))};
  chrome.downloads.download({
    url: "data:application/json;charset=utf-8," + encodeURIComponent(JSON.stringify(report, null, 2)),
    filename: "LDM-capture-diagnostic.json", conflictAction: "uniquify", saveAs: false
  }, () => void chrome.runtime.lastError);
}

chrome.action.setBadgeBackgroundColor({ color: BADGE_COLOR });

chrome.runtime.onInstalled.addListener(() => {
  chrome.contextMenus.create({
    id: MENU_DOWNLOAD_LINK,
    title: "Download link with Linux Download Manager",
    contexts: ["link"]
  });

  chrome.contextMenus.create({
    id: MENU_DOWNLOAD_PAGE,
    title: "Download page URL with Linux Download Manager",
    contexts: ["page"]
  });

  chrome.contextMenus.create({
    id: MENU_DOWNLOAD_MEDIA,
    title: "Download media with Linux Download Manager",
    contexts: ["video", "audio"]
  });
});

chrome.action.onClicked.addListener((tab) => {
  const payload = chooseBestMediaCapturePayload(
    tab?.id,
    null,
    tab?.url,
    tab?.title ?? null
  );
  if (payload?.ok) {
    queueNativeCapture(payload.capture, tab?.id);
  } else {
    notifyTab(tab?.id, "error", payload?.error ?? "Video akışı henüz yakalanamadı. Sayfayı yenileyip videoyu oynatın, ardından tekrar deneyin.");
  }
});

chrome.contextMenus.onClicked.addListener((info, tab) => {
  if (info.menuItemId === MENU_DOWNLOAD_LINK) {
    sendToNativeHost({
      url: info.linkUrl,
      sourcePageUrl: tab?.url ?? null,
      sourceTitle: tab?.title ?? null
    });
    return;
  }

  if (info.menuItemId === MENU_DOWNLOAD_PAGE) {
    const payload = chooseBestMediaCapturePayload(
      tab?.id,
      null,
      tab?.url ?? null,
      tab?.title ?? null
    );
    if (payload?.ok) {
      queueNativeCapture(payload.capture, tab?.id);
    } else {
      notifyTab(tab?.id, "error", payload?.error ?? "Video akışı henüz yakalanamadı. Sayfayı yenileyip videoyu oynatın, ardından tekrar deneyin.");
    }
    return;
  }

  if (info.menuItemId === MENU_DOWNLOAD_MEDIA) {
    sendToNativeHost({
      url: info.srcUrl,
      sourcePageUrl: tab?.url ?? null,
      sourceTitle: tab?.title ?? null
    });
  }
});

chrome.runtime.onMessage.addListener((message, sender, sendResponse) => {
  const tabId = sender.tab?.id;

  if (message?.type === "capture-diagnostic-frame") {
    if (typeof tabId === "number" && tabId >= 0) {
      diagnosticState(tabId).frames[sender.frameId ?? 0] = {
        location: diagnosticUrl(sender.url), seenAt: Date.now(),
        media: (message.media ?? []).slice(0, 12).map(m => ({source: diagnosticUrl(m.src), readyState: m.readyState, paused: m.paused})),
        embeds: (message.embeds ?? []).slice(0, 12).map(diagnosticUrl),
        resourceCount: message.resourceCount
      };
    }
    sendResponse({ok: true});
    return;
  }
  if (message?.type === "candidate-count") {
    updateBadge(tabId, message.count);
    sendResponse({ ok: true });
    return;
  }

  if (message?.type === "get-media-candidates") {
    sendResponse({
      ok: true,
      candidates: getMediaCandidatesForTab(tabId)
    });
    return;
  }

  if (message?.type === "remember-media-candidates") {
    if (typeof tabId === "number" && tabId >= 0 && Array.isArray(message.candidates)) {
      for (const candidate of message.candidates) {
        if (!candidate?.url || !shouldRememberMediaRequest(candidate.url, candidate.type ?? "observed")) {
          continue;
        }

        rememberMediaRequest(
          tabId,
          candidate.url,
          candidate.type ?? "observed",
          candidate.referrerUrl ?? sender.url ?? null,
          candidate.httpHeaders ?? {},
          true
        );
      }
    }

    sendResponse({ ok: true });
    return;
  }

  if (message?.type === "capture-download") {
    queueNativeCapture(
      {
        url: message.payload?.url,
        sourcePageUrl: message.payload?.sourcePageUrl ?? sender.tab?.url ?? null,
        sourceTitle: message.payload?.sourceTitle ?? sender.tab?.title ?? null,
        format: message.payload?.format ?? null,
        forceYtdlp: message.payload?.forceYtdlp ?? false,
        streamManifest: message.payload?.streamManifest ?? isManifestUrl(message.payload?.url)
      },
      tabId
    );
    sendResponse({ ok: true, accepted: true });
    return;
  }

  if (message?.type === "capture-best-media") {
    const payload = chooseBestMediaCapturePayload(
      tabId,
      message.payload?.preferredUrl,
      message.payload?.sourcePageUrl ?? sender.tab?.url ?? null,
      message.payload?.sourceTitle ?? sender.tab?.title ?? null
    );
    if (!payload?.ok) {
      sendResponse({
        ok: false,
        error: payload?.error ?? "Video akışı henüz yakalanamadı. Sayfayı yenileyip videoyu oynatın, ardından tekrar deneyin."
      });
      try { saveCaptureDiagnostic(tabId, sender.tab?.url); }
      catch (error) { console.warn("LDM diagnostic export failed", error); }
      return;
    }

    payload.capture.format = message.payload?.format ?? null;
    queueNativeCapture(payload.capture, tabId);
    sendResponse({
      ok: true,
      accepted: true,
      url: payload.capture.url,
      audioUrl: payload.capture.audioUrl ?? null
    });
    return;
  }
});

chrome.tabs.onRemoved.addListener((tabId) => {
  recentMediaByTab.delete(tabId);
  captureDiagnostics.delete(tabId);
  chrome.action.setBadgeText({ tabId, text: "" });
});

chrome.tabs.onUpdated.addListener((tabId, changeInfo) => {
  if (!changeInfo.url && changeInfo.status !== "loading") {
    return;
  }

  recentMediaByTab.delete(tabId);
  captureDiagnostics.delete(tabId);
  chrome.action.setBadgeText({ tabId, text: "" });
});

chrome.downloads.onCreated.addListener((downloadItem) => {
  const url = downloadItem.finalUrl || downloadItem.url;
  if (!shouldCaptureUrl(url, downloadItem.filename)) {
    return;
  }

  if (wasRecentlyCaptured(url)) {
    return;
  }

  const parsedDownloadUrl = safeParseUrl(url);
  if (parsedDownloadUrl && /\.(googlevideo\.com|ytimg\.com)$/i.test(parsedDownloadUrl.hostname)) {
    return;
  }

  if (parsedDownloadUrl && /(^|\.)(whatsapp\.com|whatsapp\.net)$/i.test(parsedDownloadUrl.hostname)) {
    return;
  }
  const referrerUrl = safeParseUrl(downloadItem.referrer || "");
  if (referrerUrl && /(^|\.)(whatsapp\.com|whatsapp\.net)$/i.test(referrerUrl.hostname)) {
    return;
  }

  chrome.downloads.cancel(downloadItem.id, () => {
    const lastError = chrome.runtime.lastError;
    if (lastError) {
      console.warn("Could not cancel browser download.", lastError.message);
      return;
    }

    setTimeout(() => {
      chrome.downloads.erase({ id: downloadItem.id }, () => void chrome.runtime.lastError);
    }, 500);
  });

  sendToNativeHost(
    {
      url,
      sourcePageUrl: downloadItem.referrer || null,
      sourceTitle: downloadItem.filename || null
    },
    downloadItem.byExtensionId ? undefined : downloadItem.tabId
  );
});

chrome.webRequest.onBeforeRequest.addListener(
  (details) => {
    if (typeof details.tabId !== "number" || details.tabId < 0) {
      return;
    }

    if (!shouldRememberMediaRequest(details.url, details.type)) {
      return;
    }

    rememberMediaRequest(
      details.tabId,
      details.url,
      details.type,
      details.documentUrl || details.initiator || null
    );
  },
  { urls: ["<all_urls>"], types: ["media", "xmlhttprequest"] }
);

// MIME evidence also identifies extensionless stream URLs.
chrome.webRequest.onHeadersReceived.addListener((details) => {
  if (details.tabId < 0) return;
  const mime = (details.responseHeaders ?? []).find(
    (header) => header.name?.toLowerCase() === "content-type"
  )?.value?.split(";")[0].trim().toLowerCase() ?? "";
  const diagnostic = diagnosticState(details.tabId);
  diagnostic.requests.push({...diagnosticUrl(details.url), frameId: details.frameId,
    type: details.type, mime, status: details.statusCode});
  diagnostic.requests = diagnostic.requests.slice(-80);
  if (details.statusCode < 200 || details.statusCode >= 300) return;
  const manifest = ["application/vnd.apple.mpegurl", "application/x-mpegurl",
    "audio/mpegurl", "audio/x-mpegurl", "application/dash+xml"].includes(mime);
  const fragment = ["video/mp2t", "video/iso.segment", "audio/iso.segment"].includes(mime);
  if (!manifest && !fragment && !/^(video|audio)\//.test(mime)) return;
  rememberMediaRequest(details.tabId, details.url,
    mime.startsWith("audio/") ? "audio" : "media", details.initiator ?? null);
  const candidate = (recentMediaByTab.get(details.tabId) ?? []).find(
    (item) => item.url === normalizeMediaCandidateUrl(details.url));
  if (candidate) {
    candidate.streamKind = candidate.bodyClassified ? "playlist" : manifest ? "playlist" : fragment ? "fragment"
      : ["unknown", "page"].includes(candidate.streamKind)
      ? (mime.startsWith("audio/") ? "audio" : "video") : candidate.streamKind;
    candidate.score = manifest ? 120 : fragment ? 30 : candidate.score;
    candidate.mimeClassified = true;
    recentMediaByTab.get(details.tabId).sort((a, b) => b.score - a.score || b.seenAt - a.seenAt);
  }
}, { urls: ["<all_urls>"], types: ["media", "xmlhttprequest"] }, ["responseHeaders"]);

function captureMediaRequestHeaders(details) {
    if (typeof details.tabId !== "number" || details.tabId < 0) {
      return;
    }

    if (!shouldRememberMediaRequest(details.url, details.type)) {
      return;
    }

    const headers = details.requestHeaders ?? [];
    const headerValue = (name) => headers.find(
      (header) => header.name?.toLowerCase() === name
    )?.value;
    const requestContext = headerValue("referer")
      || headerValue("origin")
      || details.initiator
      || null;
    const requestHeaders = {};
    for (const name of [
      "referer", "origin", "cookie", "accept", "accept-language", "user-agent",
      "sec-ch-ua", "sec-ch-ua-mobile", "sec-ch-ua-platform",
      "sec-fetch-dest", "sec-fetch-mode", "sec-fetch-site"
    ]) {
      const value = headerValue(name);
      if (value) {
        requestHeaders[name] = value;
      }
    }

    rememberMediaRequest(details.tabId, details.url, details.type, requestContext, requestHeaders);
}

const mediaHeaderFilter = { urls: ["<all_urls>"], types: ["media", "xmlhttprequest"] };
try {
  chrome.webRequest.onBeforeSendHeaders.addListener(
    captureMediaRequestHeaders,
    mediaHeaderFilter,
    ["requestHeaders", "extraHeaders"]
  );
} catch (error) {
  console.warn("extraHeaders is unavailable; using standard request headers.", error);
  chrome.webRequest.onBeforeSendHeaders.addListener(
    captureMediaRequestHeaders,
    mediaHeaderFilter,
    ["requestHeaders"]
  );
}

function sendToNativeHost(payload, tabId) {
  if (!payload?.url || !/^https?:/i.test(payload.url)) {
    const error = "Ignored a non-http URL.";
    console.warn("Linux Download Manager Bridge ignored a non-http URL.", payload);
    return Promise.resolve({
      ok: false,
      error
    });
  }

  rememberCapture(payload.url);
  if (payload.audioUrl && /^https?:/i.test(payload.audioUrl)) {
    rememberCapture(payload.audioUrl);
  }

  return new Promise((resolve) => {
    chrome.runtime.sendNativeMessage(HOST_NAME, payload, (response) => {
      if (chrome.runtime.lastError) {
        const error = chrome.runtime.lastError.message;
        console.error("Native host communication failed.", error);
        pulseBadge(tabId, "ERR");
        resolve({
          ok: false,
          error
        });
        return;
      }

      if (!response?.ok) {
        const error = response?.error ?? "Native host rejected the request.";
        console.error("Linux Download Manager rejected the request.", error);
        pulseBadge(tabId, "ERR");
        resolve({
          ok: false,
          error
        });
        return;
      }

      console.info("Linux Download Manager accepted the request.", response);
      pulseBadge(tabId, "LDM");
      resolve({
        ok: true,
        inboxFile: response?.inboxFile ?? null
      });
    });
  });
}

function queueNativeCapture(payload, tabId) {
  if (payload?.url && wasRecentlyCaptured(payload.url)) {
    notifyTab(tabId, "info", "Download is already queued in Linux Download Manager.");
    return;
  }

  enrichCaptureWithMediaCookies(payload, tabId).then((enrichedPayload) => sendToNativeHost(enrichedPayload, tabId)).then((response) => {
    if (response?.ok) {
      notifyTab(tabId, "success", "Download queued in Linux Download Manager.");
      return;
    }

    notifyTab(tabId, "error", response?.error ?? "Download request failed.");
  });
}

async function enrichCaptureWithMediaCookies(payload, tabId) {
  if (!payload?.url || !chrome.cookies?.getAll) {
    return payload;
  }

  const headers = { ...(payload.httpHeaders ?? {}) };
  const collected = [];
  const seen = new Set();
  const appendCookies = (cookies) => {
    for (const cookie of cookies ?? []) {
      const key = `${cookie.name}\u0000${cookie.domain}\u0000${cookie.path}`;
      if (!seen.has(key)) {
        seen.add(key);
        collected.push(cookie);
      }
    }
  };

  try {
    appendCookies(await chrome.cookies.getAll({ url: payload.url }));
  } catch (error) {
    console.warn("Could not read media cookies.", error);
  }

  try {
    const tab = typeof tabId === "number" ? await chrome.tabs.get(tabId) : null;
    const topPage = safeParseUrl(tab?.url ?? "");
    if (topPage) {
      appendCookies(await chrome.cookies.getAll({
        url: payload.url,
        partitionKey: { topLevelSite: topPage.origin }
      }));
    }
  } catch (error) {
    console.info("No partitioned media cookies were available.", error);
  }

  if (collected.length > 0) {
    headers.cookie = collected.map((cookie) => `${cookie.name}=${cookie.value}`).join("; ");
  }

  return { ...payload, httpHeaders: headers };
}

function shouldCaptureUrl(rawUrl, hintName = "") {
  const parsedUrl = safeParseUrl(rawUrl);
  if (!parsedUrl || !/^https?:$/i.test(parsedUrl.protocol)) {
    return false;
  }

  if (parsedUrl.searchParams.has("download")) {
    return true;
  }

  const pathname = `${parsedUrl.pathname}/${hintName}`.toLowerCase();
  const extension = pathname.split(".").pop();
  return DOWNLOADABLE_EXTENSIONS.has(extension) || STREAMING_EXTENSIONS.has(extension);
}

function safeParseUrl(rawUrl) {
  try {
    return new URL(rawUrl);
  } catch (error) {
    return null;
  }
}

function rememberCapture(url) {
  recentCaptures.set(url, Date.now());
  cleanupCaptures();
}

function wasRecentlyCaptured(url) {
  cleanupCaptures();
  const previous = recentCaptures.get(url);
  return Boolean(previous && Date.now() - previous < 4000);
}

function cleanupCaptures() {
  const now = Date.now();
  for (const [url, timestamp] of recentCaptures.entries()) {
    if (now - timestamp > 15000) {
      recentCaptures.delete(url);
    }
  }
}

function shouldRememberMediaRequest(rawUrl, type) {
  const parsedUrl = safeParseUrl(rawUrl);
  if (!parsedUrl || !/^https?:$/i.test(parsedUrl.protocol)) {
    return false;
  }

  if (type === "observed-manifest") return true;
  const pathname = parsedUrl.pathname.toLowerCase();
  const extension = pathname.split(".").pop();
  if (IMAGE_EXTENSIONS.has(extension)) {
    return false;
  }
  if (type === "media" || type === "audio") {
    return true;
  }
  if (DOWNLOADABLE_EXTENSIONS.has(extension) || STREAMING_EXTENSIONS.has(extension)) {
    return true;
  }

  if (/\/(videoplayback|manifest|playlist|master|hls|dash)\b/i.test(pathname)) {
    return true;
  }

  if (looksLikeQueryVideoUrl(parsedUrl)) {
    return true;
  }

  return /(^|\.)(fbcdn\.net|cdninstagram\.com)$/i.test(parsedUrl.hostname);
}

function isDisguisedHlsManifest(parsedUrl) {
  const pathname = parsedUrl.pathname.toLowerCase();
  return /\/hls\//.test(pathname) && /\/(master|index|playlist)\.(txt|php|json)$/.test(pathname);
}

function isManifestUrl(rawUrl) {
  const parsedUrl = safeParseUrl(rawUrl);
  if (!parsedUrl) {
    return false;
  }

  const pathname = parsedUrl.pathname.toLowerCase();
  return pathname.endsWith(".m3u8")
    || pathname.endsWith(".m3u")
    || pathname.endsWith(".mpd")
    || isDisguisedHlsManifest(parsedUrl);
}

function rememberMediaRequest(tabId, url, type, referrerUrl = null, requestHeaders = {}, observed = false) {
  const normalizedUrl = normalizeMediaCandidateUrl(url);
  if (!normalizedUrl) {
    return;
  }

  const candidates = recentMediaByTab.get(tabId) ?? [];
  const now = Date.now();
  const existingCandidate = candidates.find((candidate) => candidate.url === normalizedUrl);
  const nextCandidates = candidates.filter((candidate) => now - candidate.seenAt < 120000 && candidate.url !== normalizedUrl);
  nextCandidates.unshift({
    url: normalizedUrl,
    type,
    seenAt: now,
    score: ((existingCandidate?.mimeClassified || existingCandidate?.bodyClassified) && type !== "observed-manifest") ? existingCandidate.score : scoreMediaCandidate(url, type),
    streamKind: ((existingCandidate?.mimeClassified || existingCandidate?.bodyClassified) && type !== "observed-manifest") ? existingCandidate.streamKind : classifyMediaCandidate(url, type),
    mimeClassified: existingCandidate?.mimeClassified ?? false,
    bodyClassified: type === "observed-manifest" || existingCandidate?.bodyClassified || false,
    groupKey: deriveMediaGroupKey(url),
    referrerUrl: normalizePageUrl(referrerUrl) ?? existingCandidate?.referrerUrl ?? null,
    httpHeaders: observed
      ? { ...requestHeaders, ...(existingCandidate?.httpHeaders ?? {}) }
      : Object.keys(requestHeaders).length > 0
      ? requestHeaders
      : (existingCandidate?.httpHeaders ?? {})
  });
  nextCandidates.sort((left, right) => right.score - left.score || right.seenAt - left.seenAt);
  recentMediaByTab.set(tabId, nextCandidates.slice(0, MAX_MEDIA_CANDIDATES));
}

function getMediaCandidatesForTab(tabId) {
  if (typeof tabId !== "number" || tabId < 0) {
    return [];
  }

  const candidates = recentMediaByTab.get(tabId) ?? [];
  const now = Date.now();
  const fresh = candidates.filter((candidate) => now - candidate.seenAt < 120000);
  if (fresh.length !== candidates.length) {
    recentMediaByTab.set(tabId, fresh);
  }
  return fresh.map(({ url, type, streamKind, groupKey, referrerUrl, httpHeaders }) => ({
    url,
    type,
    streamKind,
    groupKey,
    referrerUrl,
    httpHeaders: httpHeaders ?? {}
  }));
}

function normalizePageUrl(rawUrl) {
  const parsedUrl = safeParseUrl(rawUrl);
  return parsedUrl && /^https?:$/i.test(parsedUrl.protocol) ? parsedUrl.toString() : null;
}

function scoreMediaCandidate(url, type) {
  if (type === "observed-manifest") return 125;
  const parsedUrl = safeParseUrl(url);
  if (!parsedUrl) {
    return 0;
  }

  const hasByteRange = hasByteRangeQuery(parsedUrl);
  const facebookMetadata = parseFacebookEfgMetadata(parsedUrl);
  const twitterKind = classifyTwitterMediaKind(parsedUrl);
  const extension = parsedUrl.pathname.toLowerCase().split(".").pop();
  if (IMAGE_EXTENSIONS.has(extension)) {
    return -100;
  }
  if (isDisguisedHlsManifest(parsedUrl)) {
    return 122;
  }
  if (extension === "m3u8") {
    if (twitterKind === "audio") {
      return 32;
    }
    if (twitterKind === "video") {
      return 126;
    }
    return 120;
  }
  if (extension === "mpd") {
    return 110;
  }
  if (["mp4", "mkv", "webm", "mp3", "m4a"].includes(extension)) {
    if (isAudioOnlyFacebookMetadata(facebookMetadata)) {
      return hasByteRange ? 6 : 20;
    }

    if (isVideoFacebookMetadata(facebookMetadata)) {
      return hasByteRange ? 72 : 118;
    }

    return hasByteRange ? 55 : 100;
  }
  if (looksLikeQueryVideoUrl(parsedUrl)) {
    if (isAudioOnlyFacebookMetadata(facebookMetadata)) {
      return hasByteRange ? 5 : 18;
    }

    if (isVideoFacebookMetadata(facebookMetadata)) {
      return hasByteRange ? 70 : 108;
    }

    return hasByteRange ? 45 : 95;
  }
  if ((type === "observed" || type === "media") && /(^|\.)(fbcdn\.net|cdninstagram\.com)$/i.test(parsedUrl.hostname)) {
    if (isAudioOnlyFacebookMetadata(facebookMetadata)) {
      return hasByteRange ? 4 : 16;
    }

    if (isVideoFacebookMetadata(facebookMetadata)) {
      return hasByteRange ? 68 : 102;
    }

    return hasByteRange ? 40 : 92;
  }
  if (type === "media") {
    return hasByteRange ? 38 : 80;
  }
  if (["m4s", "ts", "aac"].includes(extension)) {
    return 30;
  }
  return 20;
}

function updateBadge(tabId, count) {
  if (typeof tabId !== "number" || tabId < 0) {
    return;
  }

  const text = count > 0 ? String(Math.min(count, 99)) : "";
  chrome.action.setBadgeText({ tabId, text });
  chrome.action.setTitle({
    tabId,
    title: count > 0
      ? `Linux Download Manager found ${count} downloadable items`
      : "Linux Download Manager"
  });
}

function pulseBadge(tabId, text) {
  if (typeof tabId !== "number" || tabId < 0) {
    return;
  }

  chrome.action.setBadgeText({ tabId, text });
  setTimeout(() => {
    chrome.action.setBadgeText({ tabId, text: "" });
  }, 1800);
}

function notifyTab(tabId, tone, message) {
  if (typeof tabId !== "number" || tabId < 0) {
    return;
  }

  chrome.tabs.sendMessage(
    tabId,
    {
      type: "native-capture-status",
      tone,
      message
    },
    () => void chrome.runtime.lastError
  );
}

function looksLikeQueryVideoUrl(parsedUrl) {
  const mimeType =
    parsedUrl.searchParams.get("mime_type") ||
    parsedUrl.searchParams.get("mime") ||
    parsedUrl.searchParams.get("content_type") ||
    "";
  if (/video|audio/i.test(mimeType)) {
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
}

function normalizeMediaCandidateUrl(rawUrl) {
  const parsedUrl = safeParseUrl(rawUrl);
  if (!parsedUrl || !/^https?:$/i.test(parsedUrl.protocol)) {
    return null;
  }

  if (parsedUrl.searchParams.has("bytestart") || parsedUrl.searchParams.has("byteend")) {
    parsedUrl.searchParams.delete("bytestart");
    parsedUrl.searchParams.delete("byteend");
  }
  return parsedUrl.toString();
}

function canTryYtdlpPage(pageUrl) {
  const parsedUrl = safeParseUrl(pageUrl);
  return Boolean(parsedUrl && /^https?:$/i.test(parsedUrl.protocol)
    && !/(^|\.)yabancidizi\.news$/i.test(parsedUrl.hostname));
}

function chooseBestMediaCapturePayload(tabId, preferredUrl, sourcePageUrl, sourceTitle) {
  // X serves some captured video renditions without audio. Download from the
  // post instead so yt-dlp can select and merge the complete media.
  if (isTwitterStatusPageUrl(sourcePageUrl)) {
    return {
      ok: true,
      capture: {
        url: sourcePageUrl,
        audioUrl: null,
        sourcePageUrl,
        sourceTitle,
        forceYtdlp: true,
        streamManifest: false
      }
    };
  }

  const candidates = getMediaCandidatesForTab(tabId).filter(
    (candidate) => candidate.streamKind !== "fragment"
  );
  if (candidates.length === 0) {
    if (canTryYtdlpPage(sourcePageUrl)) {
      return {
        ok: true,
        capture: {
          url: sourcePageUrl,
          audioUrl: null,
          sourcePageUrl,
          sourceTitle,
          forceYtdlp: true,
          streamManifest: false
        }
      };
    }
    return {
      ok: false,
      error: "Video akışı henüz yakalanamadı. Sayfayı yenileyip videoyu oynatın, ardından tekrar deneyin."
    };
  }

  const normalizedPreferredUrl = preferredUrl ? normalizeMediaCandidateUrl(preferredUrl) : null;
  const preferredCandidate = normalizedPreferredUrl
    ? candidates.find((candidate) => candidate.url === normalizedPreferredUrl)
    : null;
  const twitterMasterCandidate = selectTwitterMasterCandidate(candidates, preferredCandidate);
  if (twitterMasterCandidate?.url) {
    return {
      ok: true,
      capture: {
        url: twitterMasterCandidate.url,
        fallbackUrls: [],
        audioUrl: null,
        sourcePageUrl,
        sourceTitle,
        streamManifest: true
      }
    };
  }

  const videoCandidate = selectVideoCandidate(candidates, preferredCandidate);

  const audioCandidate =
    videoCandidate && videoCandidate.streamKind !== "audio"
      ? selectAudioCompanionCandidate(candidates, videoCandidate)
      : null;

  if (!videoCandidate?.url) {
    if (canTryYtdlpPage(sourcePageUrl)) {
      return {
        ok: true,
        capture: {
          url: sourcePageUrl,
          audioUrl: null,
          sourcePageUrl,
          sourceTitle,
          forceYtdlp: true,
          streamManifest: false
        }
      };
    }
    return {
      ok: false,
      error: "Video akışı henüz yakalanamadı. Sayfayı yenileyip videoyu oynatın, ardından tekrar deneyin."
    };
  }

  if (requiresCompanionAudio(videoCandidate) && !audioCandidate?.url) {
    if (canTryYtdlpPage(sourcePageUrl)) {
      return {
        ok: true,
        capture: {
          url: sourcePageUrl,
          audioUrl: null,
          sourcePageUrl,
          sourceTitle,
          forceYtdlp: true,
          streamManifest: false
        }
      };
    }
    return {
      ok: false,
      error: "The companion audio stream was not captured. Play the video with sound for a moment, then try again."
    };
  }

  return {
    ok: true,
    capture: {
      url: videoCandidate.url,
      fallbackUrls: candidates
        .filter((candidate) =>
          candidate.url !== videoCandidate.url &&
          (candidate.streamKind === "playlist" || candidate.streamKind === "master")
        )
        .map((candidate) => candidate.url),
      audioUrl: audioCandidate?.url ?? null,
      sourcePageUrl: videoCandidate.referrerUrl ?? sourcePageUrl,
      sourceTitle,
      httpHeaders: buildMediaRequestHeaders(
        videoCandidate.httpHeaders ?? {},
        videoCandidate.referrerUrl ?? sourcePageUrl
      ),
      streamManifest: videoCandidate.streamKind === "playlist" || videoCandidate.streamKind === "master"
    }
  };
}

function isTwitterStatusPageUrl(rawUrl) {
  const url = safeParseUrl(rawUrl);
  return Boolean(url && /^https?:$/i.test(url.protocol)
    && /^(?:www\.)?(?:x|twitter)\.com$/i.test(url.hostname)
    && /\/status\/\d+(?:\/|$)/.test(url.pathname));
}

function buildMediaRequestHeaders(capturedHeaders, referrerUrl) {
  const headers = { ...(capturedHeaders ?? {}) };
  const normalizedReferrer = normalizePageUrl(referrerUrl);
  if (normalizedReferrer && !headers.referer) {
    headers.referer = normalizedReferrer;
  }
  if (normalizedReferrer && !headers.origin) {
    const parsedReferrer = safeParseUrl(normalizedReferrer);
    if (parsedReferrer) {
      headers.origin = parsedReferrer.origin;
    }
  }
  if (!headers["user-agent"] && self.navigator?.userAgent) {
    headers["user-agent"] = self.navigator.userAgent;
  }
  if (!headers.accept) {
    headers.accept = "*/*";
  }
  return headers;
}

function selectVideoCandidate(candidates, preferredCandidate) {
  const twitterMasterCandidate = selectTwitterMasterCandidate(candidates, preferredCandidate);
  if (twitterMasterCandidate) {
    return twitterMasterCandidate;
  }

  if (
    preferredCandidate &&
    ["video", "muxed", "playlist", "master"].includes(preferredCandidate.streamKind)
  ) {
    return preferredCandidate;
  }

  return candidates.find((candidate) =>
    candidate.streamKind === "video" ||
    candidate.streamKind === "muxed" ||
    candidate.streamKind === "playlist" ||
    candidate.streamKind === "master"
  ) ?? null;
}

function selectAudioCompanionCandidate(candidates, videoCandidate) {
  const compatibleCandidates = candidates.filter((candidate) =>
    candidate.streamKind === "audio" &&
    candidate.url !== videoCandidate.url
  );

  const exactGroupMatch = compatibleCandidates.find((candidate) =>
    candidate.groupKey &&
    videoCandidate.groupKey &&
    candidate.groupKey === videoCandidate.groupKey
  );
  if (exactGroupMatch) {
    return exactGroupMatch;
  }

  if (isTwitterGroupedCandidate(videoCandidate)) {
    return null;
  }

  return compatibleCandidates.find((candidate) => {
    const candidateUrl = safeParseUrl(candidate.url);
    const videoUrl = safeParseUrl(videoCandidate.url);
    return candidateUrl && videoUrl && candidateUrl.hostname === videoUrl.hostname;
  }) ?? null;
}

function classifyMediaCandidate(url, type) {
  if (type === "observed-manifest") return "playlist";
  const parsedUrl = safeParseUrl(url);
  if (!parsedUrl) {
    return "unknown";
  }

  const twitterKind = classifyTwitterMediaKind(parsedUrl);
  if (twitterKind) {
    return twitterKind;
  }

  const facebookMetadata = parseFacebookEfgMetadata(parsedUrl);
  if (isAudioOnlyFacebookMetadata(facebookMetadata)) {
    return "audio";
  }
  if (isVideoFacebookMetadata(facebookMetadata)) {
    return "video";
  }

  const extension = parsedUrl.pathname.toLowerCase().split(".").pop();
  if (/\/video\/embed\//i.test(parsedUrl.pathname)) {
    return "page";
  }
  if (IMAGE_EXTENSIONS.has(extension)) {
    return "fragment";
  }
  if (isDisguisedHlsManifest(parsedUrl)) {
    return "playlist";
  }
  if (["mp3", "m4a", "aac", "ogg", "wav"].includes(extension)) {
    return "audio";
  }
  if (["mp4", "mkv", "webm", "mov", "avi"].includes(extension)) {
    return "muxed";
  }
  if (["m3u8", "mpd"].includes(extension)) {
    return "playlist";
  }
  if (["m4s", "ts"].includes(extension)) {
    return "fragment";
  }
  if (type === "audio") {
    return "audio";
  }
  if (type === "media") {
    return "video";
  }
  return "unknown";
}

function deriveMediaGroupKey(url) {
  const parsedUrl = safeParseUrl(url);
  if (!parsedUrl) {
    return null;
  }

  const twitterGroupKey = deriveTwitterGroupKey(parsedUrl);
  if (twitterGroupKey) {
    return twitterGroupKey;
  }

  const facebookMetadata = parseFacebookEfgMetadata(parsedUrl);
  if (facebookMetadata) {
    const videoId = facebookMetadata.videoId || parsedUrl.searchParams.get("video_id");
    const assetId = facebookMetadata.assetId || parsedUrl.searchParams.get("xpv_asset_id");
    if (videoId) {
      return `fb-video:${videoId}`;
    }

    if (assetId) {
      return `fb-asset:${assetId}`;
    }
  }

  const lastSegment = parsedUrl.pathname.split("/").filter(Boolean).pop();
  return lastSegment || parsedUrl.pathname || null;
}

function classifyTwitterMediaKind(parsedUrl) {
  if (!/(^|\.)video\.twimg\.com$/i.test(parsedUrl.hostname)) {
    return null;
  }

  const pathname = parsedUrl.pathname.toLowerCase();
  if (pathname.includes("/pl/mp4a/")) {
    return "audio";
  }
  if (pathname.includes("/pl/avc1/") || pathname.includes("/pl/hevc/") || pathname.includes("/pl/h265/")) {
    return "video";
  }
  if (pathname.includes("/pl/") && pathname.endsWith(".m3u8")) {
    return "master";
  }

  return null;
}

function deriveTwitterGroupKey(parsedUrl) {
  if (!/(^|\.)video\.twimg\.com$/i.test(parsedUrl.hostname)) {
    return null;
  }

  const marker = "/pl/";
  const pathname = parsedUrl.pathname;
  const markerIndex = pathname.indexOf(marker);
  if (markerIndex === -1) {
    return `twimg:${pathname}`;
  }

  return `twimg:${pathname.slice(0, markerIndex)}`;
}

function requiresCompanionAudio(candidate) {
  return (
    typeof candidate?.groupKey === "string" &&
    candidate.groupKey.startsWith("twimg:") &&
    candidate.streamKind === "video"
  );
}

function isTwitterGroupedCandidate(candidate) {
  return typeof candidate?.groupKey === "string" && candidate.groupKey.startsWith("twimg:");
}

function selectTwitterMasterCandidate(candidates, preferredCandidate) {
  const preferredGroupKey = preferredCandidate?.groupKey;
  if (preferredGroupKey && preferredGroupKey.startsWith("twimg:")) {
    const inPreferredGroup = candidates.find((candidate) =>
      candidate.groupKey === preferredGroupKey && candidate.streamKind === "master"
    );
    if (inPreferredGroup) {
      return inPreferredGroup;
    }
  }

  return candidates.find((candidate) => candidate.streamKind === "master") ?? null;
}

function hasByteRangeQuery(parsedUrl) {
  return parsedUrl.searchParams.has("bytestart") || parsedUrl.searchParams.has("byteend");
}

function parseFacebookEfgMetadata(parsedUrl) {
  const encoded = parsedUrl.searchParams.get("efg");
  if (!encoded) {
    return null;
  }

  try {
    const normalized = encoded.replace(/-/g, "+").replace(/_/g, "/");
    const padding = "=".repeat((4 - (normalized.length % 4)) % 4);
    const decoded = JSON.parse(atob(normalized + padding));
    return {
      tag: typeof decoded?.vencode_tag === "string" ? decoded.vencode_tag.toLowerCase() : null,
      videoId: decoded?.video_id ? String(decoded.video_id) : null,
      assetId: decoded?.xpv_asset_id ? String(decoded.xpv_asset_id) : null,
      bitrate: typeof decoded?.bitrate === "number" ? decoded.bitrate : null
    };
  } catch (error) {
    return null;
  }
}

function isAudioOnlyFacebookMetadata(metadata) {
  const tag = metadata?.tag;
  return (
    typeof tag === "string" &&
    (tag.includes("audio") ||
      tag.includes("heaac") ||
      tag.includes("aac") ||
      tag.includes("opus")) &&
    !tag.includes("video")
  );
}

function isVideoFacebookMetadata(metadata) {
  const tag = metadata?.tag;
  return (
    typeof tag === "string" &&
    !isAudioOnlyFacebookMetadata(metadata) &&
    (tag.includes("video") ||
      tag.includes("av1") ||
      tag.includes("vp9") ||
      tag.includes("h264") ||
      tag.includes("hev1") ||
      tag.startsWith("dash_"))
  );
}
