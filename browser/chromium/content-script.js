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
let detectedPlayerPages = new Set();
let mediaOverlayRoot = null;
let mediaRefreshTimer = null;
let mediaScanTimer = null;
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
  window.clearTimeout(mediaScanTimer);
  clearCaptureTimeout();
  pageObserver?.disconnect();
  hoverButton?.remove();
  mediaOverlayRoot?.remove();
  for (const button of siteDownloadButtons.values()) button.remove();
  siteDownloadButtons.clear();
  showToast("The extension was reloaded. Please reload this video page too.", "info");
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
    showToast("Could not send the extension message: " + error.message, "error");
  }
}

const siteDownloadButtons = new Map();

bootstrap();

function isExcludedHost(hostname) {
  return /(^|\.)(whatsapp\.com|whatsapp\.net)$/i.test(hostname);
}

const replayedFileLinks=new WeakSet();
const pendingFileLinks=new WeakSet();
function handleFileLinkClick(event){
  if(event.button!==0||event.ctrlKey||event.metaKey||event.shiftKey||event.altKey||event.defaultPrevented)return;
  const anchor=event.composedPath().find(node=>node?.tagName==='A'&&node.href);
  if(!anchor)return;
  if(replayedFileLinks.has(anchor)){replayedFileLinks.delete(anchor);return;}
  let url;try{url=new URL(anchor.href);}catch{return;}
  if(!/^https?:$/.test(url.protocol))return;
  const fileName=anchor.getAttribute('download')||'';
  if(!/\.[a-z0-9]{1,20}$/i.test(url.pathname)&&!fileName)return;
  event.preventDefault();event.stopImmediatePropagation();
  if(pendingFileLinks.has(anchor))return;
  pendingFileLinks.add(anchor);
  sendExtensionMessage({type:'capture-file-link',url:url.href,fileName},response=>{
    pendingFileLinks.delete(anchor);
    if(response?.handled===false){
      replayedFileLinks.add(anchor);anchor.click();replayedFileLinks.delete(anchor);return;
    }
    if(chrome.runtime.lastError||!response?.ok)showToast(response?.error||chrome.runtime.lastError?.message||'Could not send the file to LDM. Please try again.','error');
  });
}

function overlayDiagnostic() {
  const inspect = element => {
    const rect = element.getBoundingClientRect();
    const style = getComputedStyle(element);
    return {tag: element.tagName, connected: element.isConnected,
      hidden: !!element.hidden, display: style.display, visibility: style.visibility,
      opacity: style.opacity, zIndex: style.zIndex,
      rect: {top: rect.top, left: rect.left, width: rect.width, height: rect.height}};
  };
  return {
    viewport: {width: innerWidth, height: innerHeight},
    documentHidden: document.hidden, fullscreen: !!document.fullscreenElement,
    disconnected: extensionDisconnected,
    layer: mediaOverlayRoot ? inspect(mediaOverlayRoot) : null,
    controls: Array.from(document.querySelectorAll('.ldm-media-button, .ldm-hover-button, .ldm-site-btn')).slice(0, 8).map(inspect),
    frames: Array.from(document.querySelectorAll('iframe')).slice(0, 12).map(frame => ({
      ...inspect(frame), recognized: !!candidateFromPlayerFrame(frame)
    }))
  };
}

function bootstrap() {
  chrome.runtime.onMessage.addListener((message, _sender, respond) => {
    if (message?.type !== "capture-overlay-diagnostic") return;
    try { respond(overlayDiagnostic()); } catch (error) { respond({error: String(error)}); }
  });
  document.addEventListener("click",handleFileLinkClick,true);
  chrome.runtime.onMessage.addListener((message,_sender,respond)=>{
    if(message?.type!=='selected-audio-language')return;
    selectedAudioLanguage().then(language=>respond({language}));return true;
  });
  if (/(^|\.)(molystream\.org|hotstream\.club|rapidvid\.org)$/i.test(window.location.hostname)) {
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

  document.addEventListener("visibilitychange", () => {
    if (!document.hidden) {
      detectLocationChange();
      scheduleCandidateReport();
      scheduleMediaRefresh();
      if (window === window.top) scheduleMediaOverlayRefresh();
    }
  });

  // Embedded players run this script too so their media can still be reported,
  // but only the top-level page should render controls. Otherwise every iframe
  // adds its own LDM button over the same visible player.
  if (window !== window.top) {
    scheduleCandidateReport();
    observePageChanges();
    reportObservedMediaCandidates();
    periodicRefreshHandle = window.setInterval(() => { if (!document.hidden) reportObservedMediaCandidates(); }, 5000);
    document.addEventListener("loadedmetadata", handleMediaSignal, true);
    document.addEventListener("play", handleMediaSignal, true);
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
  document.addEventListener("fullscreenchange", () => { refreshSiteButtonPositions(); scheduleMediaOverlayRefresh(); });
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

function positionYouTubeHeaderButton(button){
 const actions=document.querySelector('ytd-masthead #end #buttons');
 document.getElementById('ldm-youtube-toolbar')?.remove();
 if(!actions){
  if(button.parentElement!==document.documentElement)document.documentElement.appendChild(button);
  button.classList.remove('ldm-youtube-header-button');
  return false;
 }
 if(button.parentElement!==actions)actions.prepend(button);
 button.classList.add('ldm-youtube-header-button');
 button.hidden=!!document.fullscreenElement;
 for(const [name,value] of Object.entries({position:'relative',top:'auto',left:'auto',right:'auto',transform:'none'}))button.style.setProperty(name,value,'important');
 return true;
}
async function selectedAudioLanguage(){
 if(!/(^|\.)youtube\.com$/.test(location.hostname))return null;
 const requestId=crypto.randomUUID();
 return new Promise(resolve=>{
  const finish=language=>{clearTimeout(timer);window.removeEventListener('message',receive);resolve(language);};
  const receive=event=>{
   if(event.source!==window||event.origin!==location.origin||event.data?.type!=='ldm-audio-track-response'||event.data.requestId!==requestId)return;
   if(event.data.videoId!==new URL(location.href).searchParams.get('v'))return;
   const language=event.data.language;
   finish(typeof language==='string'&&/^[a-z]{2,3}(?:-[a-z0-9]{2,8})*$/i.test(language)?language:null);
  };
  const timer=setTimeout(()=>finish(null),700);
  window.addEventListener('message',receive);
  window.postMessage({type:'ldm-audio-track-request',requestId},location.origin);
 });
}
function placeAbovePlayer(button, element) {
  if(button.id==='ldm-yt-download-btn'&&positionYouTubeHeaderButton(button))return;
  const rect = element?.getBoundingClientRect();
  // Reels can start at the top edge, leaving no room above the video.
  // Place their control in the side margin instead of covering the picture.
  if(button.dataset.socialVideo==='true' && rect && rect.top<104 && rect.bottom>60 &&
      rect.width>=80 && rect.height>=60 && !document.fullscreenElement){
    const width=button.offsetWidth||174;
    let left=null;
    if(rect.right+width+16<=window.innerWidth)left=rect.right+8;
    else if(rect.left>=width+16)left=rect.left-width-8;
    if(left!==null){
      button.hidden=false;
      for(const [name,value] of Object.entries({position:'fixed',top:'64px',left:`${left}px`,right:'auto',transform:'none'}))button.style.setProperty(name,value,'important');
      return;
    }
  }
  const visible = rect && rect.width >= 80 && rect.height >= 60 &&
    rect.bottom > 48 && rect.top < window.innerHeight &&
    rect.right > 0 && rect.left < window.innerWidth && !document.fullscreenElement;
  button.hidden = !visible;
  if (!visible) return;
  button.style.setProperty("position", "fixed", "important");
  button.style.setProperty("top", `${Math.max(44, rect.top - 8)}px`, "important");
  button.style.setProperty("left", `${Math.max(8, Math.min(rect.right - 174, window.innerWidth - 182))}px`, "important");
  button.style.setProperty("right", "auto", "important");
  button.style.setProperty("transform", "translateY(-100%)", "important");
}
function refreshSiteButtonPositions() {
  for (const [anchor, button] of siteDownloadButtons) {
    if (!anchor.isConnected) { button.remove(); siteDownloadButtons.delete(anchor); }
    else placeAbovePlayer(button, anchor);
  }
}
function attachPlayerButton(button, anchor) {
  button.removeAttribute("style");
  button.classList.add("ldm-player-toolbar");
  button.textContent = "↓ Download video  ·  LDM";
  button.setAttribute("aria-label", "Download video with Linux Download Manager");
  document.documentElement.appendChild(button);
  siteDownloadButtons.set(anchor, button);
  placeAbovePlayer(button, anchor);
}

function createHoverButton() {
  // Sites with their own download control do not need a generic hover button.
  if (hasInlineButtons() || hoverButton) {
    return;
  }

  hoverButton = document.createElement("button");
  hoverButton.type = "button";
  hoverButton.className = "ldm-hover-button";
  hoverButton.textContent = "↓ Download video  ·  LDM";
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
    { label: "Use saved preferences", format: null },
    { label: "Best quality", format: "bv*+ba/b" },
    { label: "4K (2160p)", format: "bv*[height<=2160]+ba/b" },
    { label: "1440p", format: "bv*[height<=1440]+ba/b" },
    { label: "1080p", format: "bv*[height<=1080]+ba/b" },
    { label: "720p", format: "bv*[height<=720]+ba/b" },
    { label: "480p", format: "bv*[height<=480]+ba/b" },
    { label: "360p", format: "bv*[height<=360]+ba/b" },
    { label: "Download audio as MP3", format: null, outputContainer: "mp3" },
    { label: "Download audio as M4A", format: null, outputContainer: "m4a" }
  ];

  const picker = document.createElement("div");
  picker.className = "ldm-quality-picker";
  picker.setAttribute("role", "dialog");
  picker.setAttribute("aria-label", "Video download quality");
  picker.id = "ldm-quality-picker";

  const title = document.createElement("div");
  title.className = "ldm-quality-title";
  title.textContent = "Download video";
  const close = document.createElement("button");
  close.className = "ldm-quality-close";
  close.textContent = "×";
  close.setAttribute("aria-label", "Close");
  close.onclick = closeQualityPicker;
  picker.appendChild(close);
  picker.appendChild(title);
  const containerLabel=document.createElement('label');
  containerLabel.className='ldm-quality-container';
  containerLabel.textContent='Video format';
  const container=document.createElement('select');
  container.setAttribute('aria-label','Video format');
  for(const [value,label] of [['','Use saved format'],['mp4','MP4'],['mkv','MKV'],['webm','WebM']]){
    const option=document.createElement('option');option.value=value;option.textContent=label;container.appendChild(option);
  }
  containerLabel.appendChild(container);picker.appendChild(containerLabel);

  for (const quality of qualities) {
    const btn = document.createElement("button");
    btn.type = "button";
    btn.textContent = quality.label;
    btn.className = "ldm-quality-option";
    btn.addEventListener("pointerdown", (e) => e.stopPropagation());
    btn.addEventListener("click", (event) => {
      event.preventDefault();
      event.stopPropagation();
      event.stopImmediatePropagation();
      closeQualityPicker();
      triggerCaptureWithFormat(candidate, quality.format, quality.outputContainer || container.value || null);
    }, true);
    picker.appendChild(btn);
  }

  const rect = anchorElement?.getBoundingClientRect();
  if(!rect)return;
  const top=rect.bottom+6;
  picker.style.setProperty("top", top+"px", "important");
  picker.style.setProperty("left", Math.max(8,Math.min(rect.right-260,window.innerWidth-268))+"px", "important");
  picker.style.setProperty("transform", "none", "important");
  picker.style.setProperty("max-height", Math.max(40,window.innerHeight-top-8)+"px", "important");
  picker.setAttribute("role","menu");
  anchorElement.setAttribute("aria-expanded","true");
  picker.ldmAnchor=anchorElement;
  document.addEventListener("scroll",handleQualityPickerScroll,true);
  window.addEventListener("resize",closeQualityPicker);

  picker.addEventListener("keydown", event => { if (event.key === "Escape") { event.stopPropagation(); closeQualityPicker(); } });
  document.documentElement.appendChild(picker);
  close.focus({preventScroll:true});

  document.addEventListener("pointerdown", handleQualityPickerOutsideClick, true);
}

function handleQualityPickerScroll(event){
 if(!document.getElementById("ldm-quality-picker")?.contains(event.target))closeQualityPicker();
}
function closeQualityPicker() {
  document.removeEventListener("pointerdown", handleQualityPickerOutsideClick, true);
  const existing = document.getElementById("ldm-quality-picker");
  document.removeEventListener("scroll",handleQualityPickerScroll,true);
  window.removeEventListener("resize",closeQualityPicker);
  if (existing) {existing.ldmAnchor?.setAttribute("aria-expanded","false");existing.remove();}
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

async function triggerCaptureWithFormat(candidate, format, outputContainer) {
  const audioLanguage=await selectedAudioLanguage();
  if(/(^|\.)youtube\.com$/.test(location.hostname)&&!audioLanguage){showToast('Seçili ses dili okunamadı. YouTube ses parçasını seçip sayfayı yenileyerek tekrar dene.','error');return;}
  showToast("Sending download to Linux Download Manager...", "info");
  armCaptureTimeout();

  const sourcePageUrl = resolveSourcePageUrl(candidate);

  if (candidate.kind === "media" || candidate.kind === "media-fallback") {
    sendExtensionMessage({
      type: "capture-best-media",
      payload: {
        preferredUrl: candidate.url ?? null,
        playerPageUrl: candidate.playerPageUrl ?? null,
        sourcePageUrl: sourcePageUrl,
        sourceTitle: candidate.title || document.title,
        format: format ?? null,
        outputContainer: outputContainer ?? null,
        audioLanguage
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
      format: format ?? null,
        outputContainer: outputContainer ?? null,
        audioLanguage
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
    if(anchorElement?.dataset.ldmPreferencesPending==='true')return;
    if(anchorElement)anchorElement.dataset.ldmPreferencesPending='true';
    sendExtensionMessage({type:'video-menu-preferences'},response=>{
      if(anchorElement)delete anchorElement.dataset.ldmPreferencesPending;
      if(!response?.ok){showToast(response?.error||'Could not read video preferences. Open LDM and try again.','error');return;}
      if(response.showVideoQualityMenu){showQualityPicker(candidate,anchorElement);return;}
      const saved=response.preferences?.container;
      triggerCaptureWithFormat(candidate,'bv*+ba/b',['mp4','mkv','webm'].includes(saved)?saved:'mp4');
    });
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
  refreshSiteButtonPositions();
  if (mediaOverlayRoot?.childElementCount > 0) {
    if (hoverButton) hoverButton.hidden = true;
    return;
  }
  if (!hoverButton || !activeCandidate) {
    hideHoverButton();
    return;
  }

  const anchor = isStickyMediaCandidate(activeCandidate)
    ? resolveCandidateAnchorElement(activeCandidate) : activeCandidate.element;
  if (!anchor?.isConnected) { hideHoverButton(); return; }
  placeAbovePlayer(hoverButton, anchor);
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

function isExtensionMutation(record) {
  const owned = node => Boolean((node?.nodeType === 1 ? node : node?.parentElement)?.closest?.(
    '.ldm-media-layer, .ldm-hover-button, .ldm-toast-root, .ldm-site-btn, #ldm-yt-download-btn, #ldm-quality-picker, #ldm-youtube-toolbar'
  ));
  if (owned(record.target)) return true;
  const changed = [...(record.addedNodes || []), ...(record.removedNodes || [])];
  return record.type === 'childList' && changed.length > 0 && changed.every(owned);
}

function observePageChanges() {
  const observer = pageObserver = new MutationObserver(records => {
    // Updating our own overlay must not trigger another overlay update forever.
    if (document.hidden || records.every(isExtensionMutation)) return;
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
    attributeFilter: ["href", "src", "data-src", "download", "theater"]
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
    if (document.hidden) return;
    detectLocationChange();
    scheduleMediaRefresh();
    scheduleMediaOverlayRefresh();
  }, 5000);
}

function scheduleMediaRefresh() {
  // A busy social feed may mutate hundreds of times per second. Keep one
  // pending scan, rather than scheduling a full document scan per mutation.
  if (extensionDisconnected || document.hidden || mediaScanTimer !== null) return;
  mediaScanTimer = window.setTimeout(() => {
    mediaScanTimer = null;
    if (extensionDisconnected || document.hidden) return;
    reportObservedMediaCandidates();
    refreshRecentMediaCandidates();
  }, 1000);
}

function scheduleMediaOverlayRefresh() {
  if (extensionDisconnected || document.hidden || mediaRefreshTimer) return;

  mediaRefreshTimer = window.setTimeout(() => {
    mediaRefreshTimer = null;
    if (extensionDisconnected || document.hidden) return;
    refreshMediaOverlay();
  }, 1000);
}

function scheduleCandidateReport() {
  if (extensionDisconnected || document.hidden || reportTimer) return;

  reportTimer = window.setTimeout(() => {
    reportTimer = null;
    if (extensionDisconnected || document.hidden) return;
    sendExtensionMessage({
      type: "candidate-count",
      count: countCandidates()
    });
  }, 1000);
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

  refreshSiteButtonPositions();
  refreshInlineDownloadButtons();

  const host = window.location.hostname;
  if (/reddit\.com|youtube\.com|x\.com|twitter\.com|facebook\.com|instagram\.com/.test(host)) {
    return;
  }

  if (injectEmbeddedPlayerButtons()) {
    if (hoverButton) hoverButton.hidden = true;
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
    button.textContent = "↓ Download video  ·  LDM";
    if (target.pinned) {
      // Without a player anchor, a floating button could cover the video.
      continue;
    } else {
      const rect = target.element.getBoundingClientRect();
      if (!isVisibleMediaRect(rect)) {
        continue;
      }

      placeAbovePlayer(button, target.element);
      if (button.hidden) continue;
    }

    button.addEventListener("pointerdown", (event) => {
      event.preventDefault();
      event.stopPropagation();
    });
    button.addEventListener("click", (event) => {
      event.preventDefault();
      event.stopPropagation();
      activeCandidate = target.candidate;
      triggerCaptureOnce(target.candidate, button);
    });
    mediaOverlayRoot.appendChild(button);
    if (hoverButton) hoverButton.hidden = true;
    break;
  }
}

function refreshInlineDownloadButtons() {
  const host = window.location.hostname;
  if (/(^|\.)(instagram\.com|facebook\.com)$/.test(host)) injectSocialVideoButtons();
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

function cleanInstagramCaption(value) {
  if(typeof value!=='string')return '';
  const text=value.replace(/https?:\/\/\S+/gi,'').replace(/[#@][\p{L}\p{N}_.]+/gu,'')
    .replace(/[^\p{L}\p{N}\p{M}\s.,!?'-]/gu,' ').trim().split(/[\r\n]+/).find(line=>line.trim())||'';
  const words=text.replace(/\s+/g,' ').trim().split(' ').slice(0,10);
  let title=words.join(' ');
  if(title.length>80)title=title.slice(0,80).replace(/\s+\S*$/,'');
  return title.replace(/[\s.,!?'-]+$/,'');
}
function instagramVideoTitle(video, post, permalink) {
  const clean=cleanInstagramCaption;
  // Keep caption lookup scoped to this post, never another item in the feed.
  const caption=post?.querySelector('h1, [data-testid="post-caption"], [data-testid="caption"]');
  if(clean(caption?.textContent))return clean(caption.textContent);
  const ownPage=permalink && new URL(permalink).pathname.replace(/\/$/,'')===location.pathname?.replace(/\/$/,'');
  if(ownPage){
    const heading=document.querySelector?.('main h1');
    if(clean(heading?.textContent))return clean(heading.textContent);
    const description=document.querySelector?.('meta[property="og:description"]')?.content||'';
    // Instagram metadata wraps the caption in quotes after author/date statistics.
    const quoted=description.match(/:\s*["“]([\s\S]+)["”]\s*\.?$/);
    if(clean(quoted?.[1]))return clean(quoted[1]);
    const pageTitle=document.querySelector?.('meta[property="og:title"]')?.content||document.title;
    const captionInTitle=pageTitle?.match(/:\s*["“]([\s\S]+)["”]\s*\.?$/);
    if(clean(captionInTitle?.[1]))return clean(captionInTitle[1]);
  }
  return 'Instagram video';
}

function facebookVideoPermalink(raw) {
  try {
    const u=new URL(raw,location.href);
    if(!/^https?:$/.test(u.protocol)||!/(^|\.)facebook\.com$/.test(u.hostname))return null;
    const match=u.pathname.match(/\/(?:reels?|videos)\/(\d+)(?:\/|$)/);
    const id=match?.[1]||(/^\/(?:watch\/?|video\.php|reel\/?)$/.test(u.pathname)?u.searchParams.get('v'):null);
    if(id&&/^\d+$/.test(id))return `https://www.facebook.com/watch/?v=${id}`;
    if(/\/posts\/[^/]+/.test(u.pathname)||/^\/(permalink|story)\.php$/.test(u.pathname)&&u.searchParams.has('story_fbid'))return u.href;
    return null;
  }catch{return null;}
}
function facebookVideoLink(video) {
  // Stop before entering a feed container with several players.
  for(let root=video.parentElement,depth=0;root&&depth<10;root=root.parentElement,depth++){
    if(root===document.body||root.querySelectorAll('video').length>1)break;
    const urls=Array.from(root.querySelectorAll('a[href]')).map(a=>facebookVideoPermalink(a.href)).filter(Boolean);
    const unique=[...new Set(urls)];
    if(unique.length===1)return unique[0];
    if(unique.length>1)break;
  }
  return facebookVideoPermalink(location.href);
}

function facebookVideoTitle(video) {
  if(facebookVideoPermalink(location.href)){
    const metadata=document.querySelector?.('meta[property="og:title"]')?.content||'';
    const title=metadata.replace(/\s*[|–-]\s*Facebook\s*$/i,'').trim();
    const segments=title.split('|');
    const caption=segments.length>1&&/views|reactions|likes|görüntüleme|beğeni/i.test(segments[0])?segments[1]:segments[0];
    if(caption&&!/^(facebook|reels?|facebook video|video)$/i.test(caption.trim()))return cleanInstagramCaption(caption);
  }
  for(let root=video.parentElement,depth=0;root&&depth<12;root=root.parentElement,depth++){
    if(root===document.body||root.querySelectorAll('video').length>1)break;
    const caption=root.querySelector('[data-ad-preview="message"], [data-ad-comet-preview="message"], [data-testid="post_message"]');
    const title=cleanInstagramCaption(caption?.textContent);
    if(title)return title;
  }
  return 'Facebook video';
}

function socialVideoCandidate(video) {
  const instagram=/(^|\.)instagram\.com$/.test(location.hostname);
  const valid=raw=>{
    try {
      const u=new URL(raw,location.href);
      if(!/^https?:$/.test(u.protocol)||!(instagram?/(^|\.)instagram\.com$/:/(^|\.)facebook\.com$/).test(u.hostname))return false;
      return instagram?/^\/(reel|reels|p)\/[^/]+/.test(u.pathname):!!facebookVideoPermalink(u.href);
    }catch{return false;}
  };
  const post=video.closest('article, [role="article"]');
  const link=Array.from(post?.querySelectorAll('a[href]')||[]).find(a=>valid(a.href));
  const permalink=instagram?(link?.href||(valid(location.href)?location.href:null)):(facebookVideoLink(video)||facebookVideoPermalink(link?.href));
  const direct=video.currentSrc||video.src;
  if(!permalink&&!/^https?:/.test(direct||''))return null;
  return {element:video,url:permalink||direct,kind:'media-fallback',title:instagram?instagramVideoTitle(video,post,permalink):facebookVideoTitle(video)};
}
function injectSocialVideoButtons() {
  for(const video of document.querySelectorAll('video')){
    if(siteDownloadButtons.has(video))continue;
    const rect=video.getBoundingClientRect();
    if(rect.width<80||rect.height<60)continue;
    const button=document.createElement('button');button.type='button';button.className='ldm-site-btn';button.dataset.socialVideo='true';
    button.addEventListener('pointerdown',e=>{e.preventDefault();e.stopPropagation();});
    button.addEventListener('click',e=>{
      e.preventDefault();e.stopPropagation();
      const candidate=socialVideoCandidate(video);
      if(!candidate){showToast('Open this video or reel in its own page, then try again.','error');return;}
      activeCandidate=candidate;triggerCaptureForCandidate(candidate,button);
    });
    attachPlayerButton(button,video);
  }
}

function injectRedditButtons() {
  for (const player of document.querySelectorAll("shreddit-player, shreddit-player-2")) {
    if (siteDownloadButtons.has(player)) continue;

    const post = player.closest("shreddit-post, article, [class*='Post']");
    if (!post) continue;
    if (post.querySelector(".ldm-site-btn")) continue;

    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = "ldm-site-btn";
    btn.textContent = "⬇ LDM Download";

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
        sendExtensionMessage({
          type: "capture-download",
          payload: {
            url: hlsUrl,
            sourcePageUrl: window.location.href,
            sourceTitle: postTitle
          }
        }, (response) => {
          clearCaptureTimeout();
          if (chrome.runtime.lastError) {
            showToast(chrome.runtime.lastError.message, "error");
            return;
          }
          if (!response?.ok) {
            showToast(response?.error ?? "Download failed.", "error");
            return;
          }
          showToast("Download sent to Linux Download Manager", "success");
        });
      } else {
        showToast("Video URL not found.", "error");
      }
    }, true);

    attachPlayerButton(btn, player);
  }
}

function injectTwitterButtons() {
  // X renders the poster/player shell before it creates a video element.
  const containers = new Set(Array.from(document.querySelectorAll(
    '[data-testid="videoComponent"], [data-testid="videoPlayer"], video'
  ), element => element.closest('[data-testid="videoComponent"]') ||
      element.closest('[data-testid="videoPlayer"]') || element.parentElement));
  for (const videoContainer of containers) {
    if (!videoContainer || siteDownloadButtons.has(videoContainer)) continue;
    const rect = videoContainer.getBoundingClientRect();
    if (rect.width < 80 || rect.height < 60) continue;
    const btn = document.createElement('button');
    btn.type = 'button';
    btn.className = 'ldm-site-btn';
    btn.addEventListener('pointerdown', e => {
      e.preventDefault(); e.stopPropagation(); e.stopImmediatePropagation();
    }, true);
    btn.addEventListener('click', e => {
      e.preventDefault(); e.stopPropagation(); e.stopImmediatePropagation();
      // Resolve again on click: virtualized timelines can reuse player shells.
      const tweet = videoContainer.closest('article');
      const timeLink = tweet?.querySelector('a[href*="/status/"] time')?.closest('a');
      const link = timeLink || tweet?.querySelector('a[href*="/status/"]');
      const raw = link?.href || (/\/status\/\d+/.test(location.pathname) ? location.href : null);
      let postUrl = null;
      try {
        const url = new URL(raw, location.href);
        const match = url.pathname.match(/^\/([^/]+)\/status\/(\d+)/);
        if (/(^|\.)(x\.com|twitter\.com)$/.test(url.hostname) && match)
          postUrl = `${url.origin}/${match[1]}/status/${match[2]}`;
      } catch {}
      if (!postUrl) {
        showToast('Could not find this video’s post link. Open the post and try again.', 'error');
        return;
      }
      const text = tweet?.querySelector('[data-testid="tweetText"]')?.textContent?.trim();
      const user = tweet?.querySelector('[data-testid="User-Name"] a')?.textContent?.trim();
      const candidate = {
        element: videoContainer.querySelector('video') || videoContainer,
        url: postUrl,
        kind: 'media-fallback',
        title: text ? text.slice(0, 80) : (user ? `${user} video` : null)
      };
      activeCandidate = candidate;
      triggerCaptureForCandidate(candidate, btn);
    }, true);
    attachPlayerButton(btn, videoContainer);
  }
}

function injectYouTubeButton() {
  if (document.getElementById("ldm-yt-download-btn")) return;

  const player = document.querySelector("#movie_player, .html5-video-player");
  if (!player) return;

  const btn = document.createElement("button");
    btn.type = "button";
  btn.id = "ldm-yt-download-btn";
  btn.textContent = "LDM";

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
      title: document.querySelector("ytd-watch-metadata h1, #title h1")?.textContent?.trim() || document.title.replace(/\s*[-–]\s*YouTube\s*$/, ""),
      url: null,
      kind: "media-fallback"
    };
    activeCandidate = candidate;
    triggerCaptureForCandidate(candidate, btn);
  }, true);

  attachPlayerButton(btn, player);
}

function isSameCandidate(left, right) {
  return left?.url === right?.url && left?.kind === right?.kind;
}

// Cross-origin players expose their media to the extension's background worker,
// but the top page cannot inspect their video DOM. Anchor a capture button to
// the visible player frame and ask the worker for the observed stream on click.
function candidateFromPlayerFrame(frame) {
  if (!(frame instanceof Element) || frame.tagName !== "IFRAME") return null;
  const url = normalizeUrl(frame.getAttribute("src"))
    || normalizeUrl(frame.getAttribute("data-src"));
  if (!url) return null;
  const parsed = new URL(url);
  const path = parsed.pathname;
  const rapidvidPlayer = /(^|\.)rapidvid\.org$/i.test(parsed.hostname)
    && /^\/vx\/[^/]+\/?$/.test(path);
  if (!rapidvidPlayer && !detectedPlayerPages.has(url)
      && !/(?:^|\/)(?:embed|player)(?:[/.]|$)/i.test(path)) return null;
  const rect = frame.getBoundingClientRect();
  if (rect.width < 180 || rect.height < 120 || !isVisibleMediaRect(rect)) return null;
  // An embed page is HTML, not the downloadable media URL.
  return { element: frame, url: null, kind: "media-fallback", playerPageUrl: url };
}

function injectEmbeddedPlayerButtons() {
  let visible = false;
  for (const frame of document.querySelectorAll("iframe[src], iframe[data-src]")) {
    const candidate = candidateFromPlayerFrame(frame);
    const existing = siteDownloadButtons.get(frame);
    if (!candidate) {
      if (existing) { existing.remove(); siteDownloadButtons.delete(frame); }
      continue;
    }
    if (existing) {
      if (!existing.isConnected) document.documentElement.appendChild(existing);
      placeAbovePlayer(existing, frame);
      visible ||= !existing.hidden;
      continue;
    }
    const button = document.createElement("button");
    button.type = "button";
    button.className = "ldm-site-btn";
    button.addEventListener("pointerdown", event => {
      event.preventDefault(); event.stopPropagation();
    });
    button.addEventListener("click", event => {
      event.preventDefault(); event.stopPropagation();
      const current = candidateFromPlayerFrame(frame);
      if (current) { activeCandidate = current; triggerCaptureOnce(current, button); }
    });
    attachPlayerButton(button, frame);
    visible ||= !button.hidden;
  }
  return visible;
}

function collectOverlayTargets() {
  const results = [];
  const seen = new Set();

  for (const frame of document.querySelectorAll("iframe[src], iframe[data-src]")) {
    const candidate = candidateFromPlayerFrame(frame);
    if (!candidate) continue;
    seen.add(frame);
    results.push({ element: frame, candidate });
  }

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
  if (tone === 'success' || message === 'Sending download to Linux Download Manager...') return;
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
    detectedPlayerPages = new Set((response.detectedPlayerPages ?? []).map(normalizeUrl).filter(Boolean));
    if (window === window.top) scheduleMediaOverlayRefresh();
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
