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
const CAPTURE_GROUPS = {
  compressed:{label:'Archives',extensions:['7z','zip','rar','tar','gz','tgz','bz2','xz','zst','torrent']},
  video:{label:'Videos',extensions:['mp4','mkv','webm','avi','mov','m4v','m3u8','mpd']},
  music:{label:'Music',extensions:['mp3','m4a','flac','wav','ogg','opus','aac']},
  document:{label:'Documents',extensions:['pdf','epub','csv','doc','docx','xls','xlsx','ppt','pptx','odt']},
  apps:{label:'Applications',extensions:['apk','appimage','bin','deb','dmg','exe','msi','pkg','rpm']},
  image:{label:'Images',extensions:['jpg','jpeg','png','webp','gif','svg','avif','bmp']},
  iso:{label:'Disk images',extensions:['iso','img']}
};
const DEFAULT_CAPTURE_GROUPS=['compressed','video','music','document','apps','iso'];
const DEFAULT_CAPTURE_EXTENSIONS=DEFAULT_CAPTURE_GROUPS.flatMap(g=>CAPTURE_GROUPS[g].extensions);
function normalizeExtensions(values){return [...new Set(values.flatMap(v=>String(v).toLowerCase().split(/[\s,;]+/)).map(v=>v.replace(/^\./,'')).filter(v=>/^[a-z0-9]{1,20}$/.test(v)))].slice(0,300);}
function normalizeCaptureSettings(value){
  const groups=Array.isArray(value.groups)?value.groups.filter(g=>Object.hasOwn(CAPTURE_GROUPS,g)):DEFAULT_CAPTURE_GROUPS;
  const custom=String(value.custom||'').slice(0,1000);
  const extensions=normalizeExtensions(Array.isArray(value.extensions)?value.extensions:[...groups.flatMap(g=>CAPTURE_GROUPS[g].extensions),custom]);
  return {enabled:value.enabled!==false,groups,custom,extensions,excludedSites:Array.isArray(value.excludedSites)?value.excludedSites:[]};
}
let captureSettings=normalizeCaptureSettings({enabled:true});
const captureReady=(async()=>{try{const saved=await chrome.storage.local.get('captureSettings');if(saved?.captureSettings)captureSettings=normalizeCaptureSettings(saved.captureSettings);await syncCaptureRules();}catch{}})();
chrome.storage.onChanged.addListener((changes,area)=>{if(area==='local'&&changes.captureSettings?.newValue)captureSettings=normalizeCaptureSettings(changes.captureSettings.newValue);});
function captureExtensions(){return new Set(captureSettings.extensions);}
function nativeCaptureRules(payload={}){
 return new Promise(resolve=>chrome.runtime.sendNativeMessage(HOST_NAME,{action:'capture_settings',...payload},r=>resolve(chrome.runtime.lastError?{ok:false}:r)));
}
async function syncCaptureRules(){
 let result=await nativeCaptureRules();
 if(!result?.ok||!result.settings)return;
 if(!result.configured)result=await nativeCaptureRules({settings:captureSettings,initializeOnly:true});
 if(result?.ok&&result.settings){captureSettings=normalizeCaptureSettings(result.settings);await chrome.storage.local.set({captureSettings});}
}
function excludedCapture(url){
 const parsed=safeParseUrl(url);if(!parsed)return false;
 return (captureSettings.excludedSites||[]).some(pattern=>{
  const escaped=pattern.toLowerCase().split('*').map(s=>s.replace(/[.*+?^$\{\}()|[\]\\]/g,'\\$&')).join('.*');
  try{return new RegExp('^'+escaped+'$','i').test(pattern.includes('://')?parsed.href:parsed.hostname);}catch{return false;}
 });
}
const STREAMING_EXTENSIONS = new Set(["m3u8", "mpd", "m4s", "ts", "aac", "m3u"]);
const IMAGE_EXTENSIONS = new Set(["avif", "bmp", "gif", "jpeg", "jpg", "png", "svg", "webp"]);
const recentCaptures = new Map();
const recentMediaByTab = new Map();
const captureDiagnostics = new Map();
const playerFramesByTab = new Map();
function diagnosticUrl(raw) {
  try { const u = new URL(raw); return { origin: u.origin, protocol: u.protocol,
    extension: u.pathname.match(/\.([a-z0-9]{1,8})$/i)?.[1] ?? "none" }; }
  catch { return { protocol: "unknown" }; }
}
function diagnosticState(tabId) {
  if (!captureDiagnostics.has(tabId)) captureDiagnostics.set(tabId, { frames: {}, requests: [] });
  return captureDiagnostics.get(tabId);
}
async function saveCaptureDiagnostic(tabId, page) {
  let overlay;
  try { overlay = await chrome.tabs.sendMessage(tabId, {type: "capture-overlay-diagnostic"}, {frameId: 0}); }
  catch (error) { overlay = {error: String(error)}; }
  const state = diagnosticState(tabId);
  const report = {version: chrome.runtime.getManifest().version,
    page: diagnosticUrl(page), overlay, ...state,
    candidates: getMediaCandidatesForTab(tabId).map(c => ({...diagnosticUrl(c.url), kind: c.streamKind, type: c.type}))};
  chrome.downloads.download({
    url: "data:application/json;charset=utf-8," + encodeURIComponent(JSON.stringify(report, null, 2)),
    filename: "LDM-capture-diagnostic.json", conflictAction: "uniquify", saveAs: false
  }, () => void chrome.runtime.lastError);
}

chrome.action.setBadgeBackgroundColor({ color: BADGE_COLOR });

chrome.runtime.onInstalled.addListener(() => {
  chrome.contextMenus.create({id: "linux-download-manager.diagnostics", title: "Export capture diagnostics", contexts: ["action"]});
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
    notifyTab(tab?.id, "error", payload?.error ?? "The video stream could not be captured yet. Reload the page, play the video, and try again.");
  }
});

chrome.contextMenus.onClicked.addListener((info, tab) => {
  if (info.menuItemId === "linux-download-manager.diagnostics") {
    saveCaptureDiagnostic(tab?.id, tab?.url);
    return;
  }
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
      notifyTab(tab?.id, "error", payload?.error ?? "The video stream could not be captured yet. Reload the page, play the video, and try again.");
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
  if(message?.type==='popup-control' && sender.url===chrome.runtime.getURL('popup.html')) {
    (async()=>{
      await captureReady;
      if(message.settings){const next=normalizeCaptureSettings({...message.settings,excludedSites:message.settings.excludedSites??captureSettings.excludedSites});const result=await nativeCaptureRules({settings:next});if(!result?.ok||!result.settings)throw new Error('Could not save file types. Update your LDM installation.');captureSettings=normalizeCaptureSettings(result.settings);await chrome.storage.local.set({captureSettings});}else await syncCaptureRules();
      return {ok:true,settings:captureSettings,defaults:DEFAULT_CAPTURE_EXTENSIONS,groups:CAPTURE_GROUPS,media:getMediaCandidatesForTab(message.tabId).map(m=>({url:m.url,type:m.type,referrerUrl:m.referrerUrl,streamManifest:isManifestUrl(m.url)||m.type==='observed-manifest'}))};
    })().then(sendResponse,error=>sendResponse({ok:false,error:String(error)}));return true;
  }
  if(message?.type==='video-menu-preferences') {
    chrome.runtime.sendNativeMessage(HOST_NAME,{action:'media_preferences'},response=>{
      sendResponse(chrome.runtime.lastError?{ok:false,error:chrome.runtime.lastError.message}:response);
    });
    return true;
  }
  const tabId = sender.tab?.id;
  if(message?.type==='capture-file-link'){
    (async()=>{
      await captureReady;await syncCaptureRules();
      const url=message.url,source=sender.url||sender.tab?.url||null;
      if(excludedCapture(url)||excludedCapture(source)||!shouldCaptureUrl(url,message.fileName||''))return {handled:false};
      const payload=await enrichCaptureWithMediaCookies({url,sourcePageUrl:source,
        sourceTitle:message.fileName||null,httpHeaders:source?{referer:source}:{}},tabId);
      const result=await sendToNativeHost(payload,tabId);
      return {handled:true,ok:!!result?.ok,error:result?.error};
    })().then(sendResponse,error=>sendResponse({handled:true,ok:false,error:String(error)}));return true;
  }

  if (message?.type === "capture-diagnostic-frame") {
    if (typeof tabId === "number" && tabId >= 0) {
      rememberPlayerFrames(tabId, sender.frameId ?? 0, sender.url, message.embeds ?? []);
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
        outputContainer: message.payload?.outputContainer ?? null,
        audioLanguage: message.payload?.audioLanguage ?? null,
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
      message.payload?.sourceTitle ?? sender.tab?.title ?? null,
      message.payload?.playerPageUrl ?? null
    );
    if (!payload?.ok) {
      sendResponse({
        ok: false,
        error: payload?.error ?? "The video stream could not be captured yet. Reload the page, play the video, and try again."
      });
      return;
    }

    payload.capture.format = message.payload?.format ?? null;
    payload.capture.outputContainer = message.payload?.outputContainer ?? null;
    payload.capture.audioLanguage = message.payload?.audioLanguage ?? null;
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
  playerFramesByTab.delete(tabId);
  chrome.action.setBadgeText({ tabId, text: "" });
});

chrome.tabs.onUpdated.addListener((tabId, changeInfo) => {
  if (!changeInfo.url && changeInfo.status !== "loading") {
    return;
  }

  recentMediaByTab.delete(tabId);
  captureDiagnostics.delete(tabId);
  playerFramesByTab.delete(tabId);
  chrome.action.setBadgeText({ tabId, text: "" });
});

chrome.downloads.onCreated.addListener(async (downloadItem) => {
  await captureReady;
  if(downloadItem.byExtensionId)return;
  await syncCaptureRules();
  if(!captureSettings.enabled)return;
  const url = downloadItem.finalUrl || downloadItem.url;
  if (excludedCapture(url) || excludedCapture(downloadItem.referrer) || !shouldCaptureUrl(url, downloadItem.filename)) {
    return;
  }

  if (wasRecentlyCaptured(url)) {
    // The desktop already accepted this URL (for example from a link click).
    // A second browser request must not keep downloading the same file.
    try {
      await chrome.downloads.cancel(downloadItem.id);
      await chrome.downloads.erase({id:downloadItem.id});
    } catch (error) {
      console.warn('Could not cancel duplicate browser download:', String(error));
    }
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

  // Keep the browser copy resumable until the desktop has accepted a job.
  try { await chrome.downloads.pause(downloadItem.id); } catch { return; }
  try {
    const payload=await enrichCaptureWithMediaCookies({url,sourcePageUrl:downloadItem.referrer||null,
      sourceTitle:(downloadItem.filename||'').split(/[\\/]/).pop()||null,
      httpHeaders:downloadItem.referrer?{referer:downloadItem.referrer}:{}},downloadItem.tabId);
    const response=await sendToNativeHost(payload,downloadItem.tabId);
    if(!response?.ok)throw new Error(response?.error||'Application unavailable');
    await chrome.downloads.cancel(downloadItem.id);
    await chrome.downloads.erase({id:downloadItem.id});
  } catch(error) {
    try {await chrome.downloads.resume(downloadItem.id);}catch{}
    console.warn('Browser download retained:',String(error));
  }
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
    candidate.streamKind = vidramePlaylistKind(details.url) || (candidate.bodyClassified ? "playlist" : manifest ? "playlist" : fragment ? "fragment"
      : ["unknown", "page"].includes(candidate.streamKind)
      ? (mime.startsWith("audio/") ? "audio" : "video") : candidate.streamKind);
    candidate.score = vidramePlaylistKind(details.url) ? scoreMediaCandidate(details.url, candidate.type) : manifest ? 120 : fragment ? 30 : candidate.score;
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

async function sendToNativeHost(payload, tabId) {
  if (payload?.outputContainer != null || payload?.audioLanguage != null) {
    if (payload.outputContainer!=null&&!['mp4','mkv','webm','mp3','m4a'].includes(payload.outputContainer)) return {ok:false,error:'Unsupported media container'};
    const settings=await new Promise(resolve=>chrome.runtime.sendNativeMessage(HOST_NAME,{action:'media_preferences'},r=>resolve(chrome.runtime.lastError?{ok:false,error:chrome.runtime.lastError.message}:r)));
    if(!settings?.ok || !settings.preferences)return {ok:false,error:settings?.error||'Media preferences unavailable; restart LDM after updating'};
    if(payload.audioLanguage!=null&&!/^[a-z]{2,3}(?:-[a-z0-9]{2,8})*$/i.test(payload.audioLanguage))return {ok:false,error:'Invalid audio language'};
    payload={...payload,transfer:{...settings.preferences,container:payload.outputContainer||settings.preferences.container,audioLanguage:payload.audioLanguage||null}};
    delete payload.audioLanguage;
    delete payload.outputContainer;
  }
  if (!payload?.url || !/^https?:/i.test(payload.url)) {
    const error = "Ignored a non-http URL.";
    console.warn("Linux Download Manager Bridge ignored a non-http URL.", payload);
    return Promise.resolve({
      ok: false,
      error
    });
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

      rememberCapture(payload.url);
      if(payload.audioUrl)rememberCapture(payload.audioUrl);
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
  if(!captureSettings.enabled)return false;
  const parsedUrl=safeParseUrl(rawUrl);
  if(!parsedUrl||!/^https?:$/i.test(parsedUrl.protocol))return false;
  const allowed=captureExtensions();
  const extension=value=>value.split(/[\\/]/).pop().toLowerCase().split('.').pop();
  return allowed.has(extension(hintName))||allowed.has(extension(parsedUrl.pathname));
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

  return /(^|\.)(fbcdn\.net|cdninstagram\.com|redd\.it|redditmedia\.com|redditstatic\.com)$/i.test(parsedUrl.hostname);
}

// This player serves HLS playlists as .txt, including separate audio renditions.
function vidramePlaylistKind(rawUrl) {
 const u=safeParseUrl(rawUrl);
 if(u && /(^|\.)rapidrame\.com$/i.test(u.hostname) && /\.urlset\//.test(u.pathname)) {
   const name=u.pathname.split('/').pop();
   if(/^master\.m3u8$/i.test(name))return 'master';
   if(/^index-f\d+-a\d+\.m3u8$/i.test(name))return 'audio';
   if(/^index-f\d+-v\d+(?:-a\d+)?\.m3u8$/i.test(name))return 'playlist';
 }
 if(u && /\/hls\//i.test(u.pathname)) {
  const name=u.pathname.split('/').pop();
  if(/^master\.(txt|m3u8)$/i.test(name))return 'master';
  // Audio renditions are published as their own playlist; they must never win a video pick.
  if(/^(sublist_)?aud\d*\.(txt|m3u8)$/i.test(name))return 'audio';
  if(/^sublist_?\d+\.(txt|m3u8)$/i.test(name))return 'playlist';
 }
 if(!u || !/(^|\.)vidrame\.(live|pro)$/i.test(u.hostname) || !/^\/vr\/get\/[^/]+\//.test(u.pathname))return null;
 const name=u.pathname.split('/').pop();
 if(/^master\.txt$/i.test(name))return 'master';
 if(/^audio-[^/]+\.txt$/i.test(name))return 'audio';
 if(/^\d+\.txt$/.test(name))return 'playlist';
 return null;
}

function isDisguisedHlsManifest(parsedUrl) {
  const pathname = parsedUrl.pathname.toLowerCase();
  if(vidramePlaylistKind(parsedUrl.href))return true;
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
  const role=vidramePlaylistKind(url);
  if(role)return role==='master'?140:role==='audio'?32:120;
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

// Keep the frame tree reported by our all-frames content script. A selected
// outer player may embed the actual streaming player on another origin.
function rememberPlayerFrames(tabId, frameId, pageUrl, embeds) {
  const page = normalizePageUrl(pageUrl);
  if (!page) return;
  if (!playerFramesByTab.has(tabId)) playerFramesByTab.set(tabId, new Map());
  const frames = playerFramesByTab.get(tabId);
  frames.set(frameId, {page, children: embeds.slice(0, 12).map(normalizePageUrl).filter(Boolean)});
  if (frames.size > 64) frames.delete(frames.keys().next().value);
}

function candidateBelongsToPlayer(candidate, playerPageUrl, tabId) {
  const referrer = safeParseUrl(candidate.referrerUrl);
  if (!referrer) return false;
  const frames = playerFramesByTab.get(tabId);
  const pending = [normalizePageUrl(playerPageUrl)];
  const visited = new Set();
  while (pending.length && visited.size < 64) {
    const page = pending.pop();
    if (!page || visited.has(page)) continue;
    visited.add(page);
    const player = safeParseUrl(page);
    if (player && player.origin === referrer.origin &&
        (referrer.pathname === "/" ||
         (referrer.pathname === player.pathname && referrer.search === player.search))) return true;
    if (frames) for (const frame of frames.values()) {
      if (frame.page === page) pending.push(...frame.children);
    }
  }
  return false;
}

function chooseBestMediaCapturePayload(tabId, preferredUrl, sourcePageUrl, sourceTitle, playerPageUrl = null) {
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

  // Resolve a specific social post, rather than another video's cached CDN URL.
  const socialUrl=safeParseUrl(sourcePageUrl);
  if(socialUrl && (
    /(^|\.)instagram\.com$/.test(socialUrl.hostname)&&/^\/(reel|reels|p)\/[^/]+/.test(socialUrl.pathname) ||
    /(^|\.)facebook\.com$/.test(socialUrl.hostname)&&(/\/(reels?|videos)\/\d+(?:\/|$)/.test(socialUrl.pathname)||/^\/(watch\/?|video\.php|reel\/?)$/.test(socialUrl.pathname)&&/^\d+$/.test(socialUrl.searchParams.get('v')||'')||/\/posts\/[^/]+/.test(socialUrl.pathname)||/^\/(permalink|story)\.php$/.test(socialUrl.pathname)&&socialUrl.searchParams.has('story_fbid'))
  )) return {ok:true,capture:{url:sourcePageUrl,audioUrl:null,sourcePageUrl,sourceTitle,forceYtdlp:true,streamManifest:false}};

  // Reddit videos use DASH with separate video+audio streams.
  // yt-dlp handles the merging automatically.
  if (isRedditPageUrl(sourcePageUrl)) {
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
      && (!playerPageUrl || candidateBelongsToPlayer(candidate, playerPageUrl, tabId))
  );
  // Never fall back to extracting the outer page: it may contain a trailer or ad.
  if (playerPageUrl && candidates.length === 0) {
    return { ok: false, error: "The selected player's video stream has not been detected. Reload the page, play the main video, and try again." };
  }
  if (candidates.length === 0) {
    if (!playerPageUrl && canTryYtdlpPage(sourcePageUrl)) {
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
      error: "The video stream could not be captured yet. Reload the page, play the video, and try again."
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
    videoCandidate && !["audio", "master"].includes(videoCandidate.streamKind)
      ? selectAudioCompanionCandidate(candidates, videoCandidate)
      : null;

  if (!videoCandidate?.url) {
    if (!playerPageUrl && canTryYtdlpPage(sourcePageUrl)) {
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
      error: "The video stream could not be captured yet. Reload the page, play the video, and try again."
    };
  }

  if (requiresCompanionAudio(videoCandidate) && !audioCandidate?.url) {
    if (!playerPageUrl && canTryYtdlpPage(sourcePageUrl)) {
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
      fallbackUrls: videoCandidate.streamKind === "master" ? [] : candidates
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

function isRedditPageUrl(rawUrl) {
  const url = safeParseUrl(rawUrl);
  return Boolean(url && /^https?:$/i.test(url.protocol)
    && /(^|\.)reddit\.com$/i.test(url.hostname)
    && /\/comments\//i.test(url.pathname));
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
  const master=candidates.find(c=>vidramePlaylistKind(c.url)==='master' && (!preferredCandidate || new URL(c.url).pathname.replace(/[^/]+$/,'')===new URL(preferredCandidate.url).pathname.replace(/[^/]+$/,'') && new URL(c.url).origin===new URL(preferredCandidate.url).origin));
  if(master)return master;
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
  const role=vidramePlaylistKind(url);
  if(role)return role;
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
