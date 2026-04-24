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
const recentCaptures = new Map();
const recentMediaByTab = new Map();

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
  sendToNativeHost({
    url: tab?.url,
    sourcePageUrl: tab?.url,
    sourceTitle: tab?.title ?? null
  });
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
    sendToNativeHost({
      url: tab?.url,
      sourcePageUrl: tab?.url ?? null,
      sourceTitle: tab?.title ?? null
    });
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

        rememberMediaRequest(tabId, candidate.url, candidate.type ?? "observed");
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
        format: message.payload?.format ?? null
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
        error: payload?.error ?? "No captured media stream found for this tab yet."
      });
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

    rememberMediaRequest(details.tabId, details.url, details.type);
  },
  { urls: ["<all_urls>"], types: ["media", "xmlhttprequest"] }
);

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
  sendToNativeHost(payload, tabId).then((response) => {
    if (response?.ok) {
      notifyTab(tabId, "success", "Download queued in Linux Download Manager.");
      return;
    }

    notifyTab(tabId, "error", response?.error ?? "Download request failed.");
  });
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

  if (type === "media") {
    return true;
  }

  const pathname = parsedUrl.pathname.toLowerCase();
  const extension = pathname.split(".").pop();
  if (DOWNLOADABLE_EXTENSIONS.has(extension) || STREAMING_EXTENSIONS.has(extension)) {
    return true;
  }

  if (/\/(videoplayback|manifest|playlist|master|video|hls|dash)\b/i.test(pathname)) {
    return true;
  }

  if (looksLikeQueryVideoUrl(parsedUrl)) {
    return true;
  }

  return /(^|\.)(fbcdn\.net|cdninstagram\.com)$/i.test(parsedUrl.hostname);
}

function rememberMediaRequest(tabId, url, type) {
  const normalizedUrl = normalizeMediaCandidateUrl(url);
  if (!normalizedUrl) {
    return;
  }

  const candidates = recentMediaByTab.get(tabId) ?? [];
  const now = Date.now();
  const nextCandidates = candidates.filter((candidate) => now - candidate.seenAt < 120000 && candidate.url !== normalizedUrl);
  nextCandidates.unshift({
    url: normalizedUrl,
    type,
    seenAt: now,
    score: scoreMediaCandidate(url, type),
    streamKind: classifyMediaCandidate(url, type),
    groupKey: deriveMediaGroupKey(url)
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
  return fresh.map(({ url, type, streamKind, groupKey }) => ({ url, type, streamKind, groupKey }));
}

function scoreMediaCandidate(url, type) {
  const parsedUrl = safeParseUrl(url);
  if (!parsedUrl) {
    return 0;
  }

  const hasByteRange = hasByteRangeQuery(parsedUrl);
  const facebookMetadata = parseFacebookEfgMetadata(parsedUrl);
  const twitterKind = classifyTwitterMediaKind(parsedUrl);
  const extension = parsedUrl.pathname.toLowerCase().split(".").pop();
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

  parsedUrl.searchParams.delete("bytestart");
  parsedUrl.searchParams.delete("byteend");
  return parsedUrl.toString();
}

function isYtdlpSupportedPage(pageUrl) {
  if (!pageUrl) return false;
  return /(facebook\.com|fb\.watch|instagram\.com|x\.com|twitter\.com|youtube\.com|youtu\.be)\//i.test(pageUrl);
}

function chooseBestMediaCapturePayload(tabId, preferredUrl, sourcePageUrl, sourceTitle) {
  const candidates = getMediaCandidatesForTab(tabId);
  if (candidates.length === 0) {
    if (isYtdlpSupportedPage(sourcePageUrl)) {
      return {
        ok: true,
        capture: {
          url: sourcePageUrl,
          audioUrl: null,
          sourcePageUrl,
          sourceTitle
        }
      };
    }
    return {
      ok: false,
      error: "No captured media stream found for this tab yet."
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
        audioUrl: null,
        sourcePageUrl,
        sourceTitle
      }
    };
  }

  const videoCandidate =
    selectVideoCandidate(candidates, preferredCandidate) ??
    preferredCandidate ??
    candidates[0];

  const audioCandidate =
    videoCandidate && videoCandidate.streamKind !== "audio"
      ? selectAudioCompanionCandidate(candidates, videoCandidate)
      : null;

  if (!videoCandidate?.url) {
    if (isYtdlpSupportedPage(sourcePageUrl)) {
      return {
        ok: true,
        capture: {
          url: sourcePageUrl,
          audioUrl: null,
          sourcePageUrl,
          sourceTitle
        }
      };
    }
    return {
      ok: false,
      error: "No captured media stream found for this tab yet."
    };
  }

  if (requiresCompanionAudio(videoCandidate) && !audioCandidate?.url) {
    if (isYtdlpSupportedPage(sourcePageUrl)) {
      return {
        ok: true,
        capture: {
          url: sourcePageUrl,
          audioUrl: null,
          sourcePageUrl,
          sourceTitle
        }
      };
    }
    return {
      ok: false,
      error: "X audio stream not captured yet. Play the video with sound for a moment, then try again."
    };
  }

  return {
    ok: true,
    capture: {
      url: videoCandidate.url,
      audioUrl: audioCandidate?.url ?? null,
      sourcePageUrl,
      sourceTitle
    }
  };
}

function selectVideoCandidate(candidates, preferredCandidate) {
  const twitterMasterCandidate = selectTwitterMasterCandidate(candidates, preferredCandidate);
  if (twitterMasterCandidate) {
    return twitterMasterCandidate;
  }

  if (preferredCandidate && preferredCandidate.streamKind !== "audio") {
    return preferredCandidate;
  }

  return candidates.find((candidate) =>
    candidate.streamKind === "video" ||
    candidate.streamKind === "muxed" ||
    candidate.streamKind === "playlist" ||
    candidate.streamKind === "unknown"
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
  if (["mp3", "m4a", "aac", "ogg", "wav"].includes(extension)) {
    return "audio";
  }
  if (["mp4", "mkv", "webm", "mov", "avi"].includes(extension)) {
    return "muxed";
  }
  if (["m3u8", "mpd"].includes(extension)) {
    return "playlist";
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
