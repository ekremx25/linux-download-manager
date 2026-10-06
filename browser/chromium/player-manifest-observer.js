// Observe playlist responses already fetched by this player. No extra requests.
(() => {
  const limit = 256 * 1024;
  const manifests = new Map();
  function publish(url, value) {
    if (typeof value !== 'string' || !/^\s*#EXTM3U\b/.test(value)
        || !/#EXT(?:INF|-X-STREAM-INF|-X-TARGETDURATION):/.test(value)) return;
    try {
      const resolved = new URL(url, location.href);
      if (!/^https?:$/.test(resolved.protocol)) return;
      manifests.set(resolved.href, true);
      if (manifests.size > 12) manifests.delete(manifests.keys().next().value);
      window.postMessage({type: 'ldm-player-manifest', url: resolved.href}, location.origin);
    } catch (_) {}
  }
  window.addEventListener('message', event => {
    if (event.source !== window || event.data?.type !== 'ldm-request-player-manifests') return;
    for (const url of manifests.keys()) {
      window.postMessage({type: 'ldm-player-manifest', url}, location.origin);
    }
  });
  const originalFetch = window.fetch;
  if (originalFetch) window.fetch = function (...args) {
    const result = Reflect.apply(originalFetch, this, args);
    result.then(response => {
      const mime = response.headers.get('content-type') || '';
      if (!response.ok || /^(image|video|audio)\//i.test(mime)
          && !/mpegurl/i.test(mime)) return;
      if (Number(response.headers.get('content-length')) > limit) return;
      const copy = response.clone();
      (async () => {
        const reader = copy.body?.getReader();
        if (!reader) return;
        const decoder = new TextDecoder();
        let size = 0, text = '';
        try {
          while (true) {
            const {value, done} = await reader.read();
            if (done) break;
            size += value.byteLength;
            if (size > limit) return;
            text += decoder.decode(value, {stream: true});
            if (text.length > 32 && !/^\s*#EXTM3U\b/.test(text)) return;
          }
          text += decoder.decode();
          publish(response.url, text);
        } finally { reader.cancel().catch(() => {}); }
      })().catch(() => {});
    }).catch(() => {});
    return result;
  };
  const originalOpen = XMLHttpRequest.prototype.open;
  XMLHttpRequest.prototype.open = function (...args) {
    if (!this.__ldmPlaylistObserved) {
      this.__ldmPlaylistObserved = true;
      this.addEventListener('load', () => {
        try {
          if (this.status < 200 || this.status >= 300) return;
          if (!this.responseType || this.responseType === 'text') {
            if (this.responseText.length <= limit) publish(this.responseURL, this.responseText);
          } else if (this.responseType === 'arraybuffer' && this.response?.byteLength <= limit) {
            publish(this.responseURL, new TextDecoder().decode(this.response));
          }
        } catch (_) {}
      });
    }
    return Reflect.apply(originalOpen, this, args);
  };
})();
