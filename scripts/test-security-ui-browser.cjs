// Run from the repository root: node scripts/test-security-ui-browser.cjs
// Requires existing Playwright; use NODE_PATH when installed outside this project.
// Uses a fresh, ephemeral browser context. All HTTP requests are fulfilled locally;
// no native host, real user profile, application backend or external server is used.
const fs = require('node:fs');
const path = require('node:path');
const assert = require('node:assert/strict');
const {chromium} = require('playwright');
const root = path.resolve(__dirname, '..');
const read = name => fs.readFileSync(path.join(root, name), 'utf8');
const contentScript = read('browser/chromium/content-script.js').replace('\nbootstrap();', '');
const csp = JSON.parse(read('src-tauri/tauri.conf.json')).app.security.csp;
const origin = 'http://ldm.test';

async function captureControl(page, kind) {
  await page.goto(`${origin}/capture-fixture.html`);
  await page.evaluate(() => { window.chrome.runtime = {id: 'test'}; });
  await page.addScriptTag({content: contentScript});
  const selector = await page.evaluate(kind => {
    window.nativePayloads = [];
    showToast = () => {};
    armCaptureTimeout = () => {};
    clearCaptureTimeout = () => {};
    pulseHoverButton = () => {};
    selectedAudioLanguage = async () => 'en';
    sendExtensionMessage = (message, callback) => {
      if (message.type.startsWith('capture-')) window.nativePayloads.push(message);
      callback?.({ok: true, options: [], showVideoQualityMenu: false, preferences: {container: 'mp4'}});
    };
    // Isolate discovery/layout; keep production DOM callbacks and capture pipeline.
    hasInlineButtons = () => false;
    placeAbovePlayer = button => { button.hidden = false; button.style.cssText = 'position:fixed;top:8px;left:8px;z-index:999'; };
    const player = document.createElement('video');
    player.id = 'player';
    player.style.cssText = 'display:block;width:400px;height:220px';
    document.body.append(player);
    const candidate = {element: player, kind: 'media', url: 'https://example.com/movie.mp4'};
    if (kind === 'file') {
      const anchor = document.createElement('a');
      anchor.href = 'https://example.com/archive.zip';
      anchor.id = 'file'; anchor.textContent = 'Download file';
      document.body.append(anchor);
      document.addEventListener('click', handleFileLinkClick, true);
      document.addEventListener('click', event => event.preventDefault());
      return '#file';
    }
    if (kind === 'hover') {
      createHoverButton(); activeCandidate = candidate; hoverButton.hidden = false;
      return '.ldm-hover-button';
    }
    if (kind === 'quality') {
      showQualityPicker(candidate, player);
      return '.ldm-quality-option';
    }
    if (kind === 'stream') {
      showDetectedMediaPicker(candidate, player, [{label: 'Video stream', url: candidate.url}]);
      return '.ldm-quality-option';
    }
    if (kind === 'generic') {
      createMediaOverlay();
      refreshInlineDownloadButtons = () => {};
      injectEmbeddedPlayerButtons = () => false;
      collectOverlayTargets = () => [{element: player, candidate}];
      resolveActiveOverlayTarget = () => null;
      resolvePersistentMediaCandidate = () => null;
      refreshMediaOverlay();
      return '.ldm-media-button';
    }
    if (kind === 'social') {
      socialVideoCandidate = () => candidate;
      injectSocialVideoButtons();
      return '.ldm-site-btn';
    }
    if (kind === 'reddit') {
      const post = document.createElement('shreddit-post');
      const reddit = document.createElement('shreddit-player');
      reddit.setAttribute('src', 'https://v.redd.it/test/video.m3u8');
      reddit.style.cssText = 'display:block;width:400px;height:220px';
      post.append(reddit); document.body.append(post);
      injectRedditButtons();
      return '.ldm-site-btn';
    }
    if (kind === 'twitter') {
      player.remove(); // Only the tweet player belongs to this fixture.
      const article = document.createElement('article');
      article.innerHTML = '<a href="https://x.com/test/status/123"><time>Now</time></a><div data-testid="videoComponent" style="width:400px;height:220px"></div>';
      document.body.append(article);
      injectTwitterButtons();
      return '.ldm-site-btn';
    }
    if (kind === 'youtube') {
      youtubePlayer = () => player;
      injectYouTubeButton();
      return '#ldm-yt-download-btn';
    }
    const frame = document.createElement('iframe');
    frame.src = '/embed/player'; frame.style.cssText = 'width:400px;height:220px';
    document.body.prepend(frame);
    injectEmbeddedPlayerButtons();
    return '.ldm-site-btn';
  }, kind);
  const button = page.locator(selector).first();
  assert.equal(await button.count(), 1, `${kind}: control exists`);
  await button.evaluate(async element => {
    element.click();
    element.dispatchEvent(new MouseEvent('click', {bubbles: true, cancelable: true}));
    await Promise.resolve();
  });
  assert.equal(await page.evaluate(() => nativePayloads.length), 0, `${kind}: synthetic clicks cannot capture`);
  await button.click();
  await page.waitForFunction(() => nativePayloads.length === 1);
  assert.equal(await page.evaluate(() => nativePayloads.length), 1, `${kind}: genuine click captures exactly once`);
  console.log(`PASS: browser-native trust boundary: ${kind}`);
}

async function popupControls(page) {
  await page.goto(`${origin}/capture-fixture.html`);
  await page.evaluate(() => {
    for (const id of ['error','connection','download-list','no-downloads','media-list','no-media','capture-enabled','extensions','extension-count','media-container','media-height','save-media','media-saved','saved','custom','add-extension','reset-types','save','open','page-video']) {
      const element = document.createElement(id === 'media-list' || id === 'download-list' ? 'div' : 'button');
      element.id = id; document.body.append(element);
    }
    window.nativePayloads = [];
    chrome.runtime = {
      lastError: null,
      async sendMessage() { return {ok: true, settings: {enabled: true, extensions: ['zip']}, defaults: ['zip'], media: [{url: 'https://example.com/movie.mp4'}]}; },
      sendNativeMessage(_host, payload, callback) {
        if (payload.url) nativePayloads.push(payload);
        callback({ok: true, preferences: {container: 'mp4', maxHeight: 1080}, downloads: []});
      }
    };
    chrome.tabs = {query: async () => [{id: 1, url: 'https://example.com/video', title: 'Video'}]};
    chrome.cookies = {getAll: async () => []};
  });
  await page.addScriptTag({content: read('browser/chromium/popup.js')});
  await page.waitForSelector('#media-list button');
  for (const selector of ['#media-list button', '#page-video']) {
    const button = page.locator(selector);
    await button.evaluate(async element => { element.click(); await Promise.resolve(); });
    assert.equal(await page.evaluate(() => nativePayloads.length), 0, `${selector}: synthetic popup capture rejected`);
    await button.click();
    await page.waitForFunction(() => nativePayloads.length === 1);
    await page.evaluate(() => { nativePayloads.length = 0; });
  }
  console.log('PASS: popup capture controls retain genuine clicks only');
}

async function uiChecks(page) {
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.goto(`${origin}/index.html`);
  const rows = page.locator('.download-row.item');
  await rows.first().waitFor();
  assert.equal(await page.locator('.table-head > [role="columnheader"]').count(), 7);
  assert.equal(await rows.first().locator(':scope > [role="cell"]').count(), 7);
  assert.equal(await page.locator('#download-rows').getAttribute('role'), 'rowgroup');
  await rows.first().focus();
  await page.keyboard.press('Shift+F10');
  const enabled = page.locator('#context-menu [role="menuitem"]:not(:disabled)');
  assert.equal(await enabled.first().evaluate(e => e === document.activeElement), true);
  await page.keyboard.press('End');
  assert.equal(await enabled.last().evaluate(e => e === document.activeElement), true);
  await page.keyboard.press('ArrowDown');
  assert.equal(await enabled.first().evaluate(e => e === document.activeElement), true);
  await page.keyboard.press('ArrowUp');
  assert.equal(await enabled.last().evaluate(e => e === document.activeElement), true);
  await page.keyboard.press('Home');
  assert.equal(await enabled.first().evaluate(e => e === document.activeElement), true);
  await page.keyboard.press('Escape');
  assert.equal(await page.locator('#context-menu').isHidden(), true);
  assert.equal(await rows.first().evaluate(e => e === document.activeElement), true);
  await page.keyboard.press('ArrowDown');
  assert.equal(await rows.nth(1).evaluate(e => e === document.activeElement), true);
  for (const selector of ['#add-url', '.sort-button', '.row-actions button', '#search-input']) {
    const control = page.locator(selector).first();
    await control.focus(); await page.keyboard.press('ArrowDown');
    assert.equal(await control.evaluate(e => e === document.activeElement), true, `${selector}: row navigation must not steal focus`);
  }
  // Policy enforcement in an actual browser: self scripts run, inline/eval/remote do not.
  await page.evaluate(src => new Promise((resolve, reject) => {
    const script = document.createElement('script'); script.src = src;
    script.onload = () => resolve(); script.onerror = () => reject(new Error('Same-origin CSP probe did not load'));
    document.head.append(script);
  }), `${origin}/csp-probe.js`);
  await page.waitForFunction(() => window.cspProbe.remoteBlocked);
  assert.deepEqual(await page.evaluate(() => window.cspProbe), {local: true, evalBlocked: true, inlineRan: false, remoteBlocked: true});
  await page.evaluate(() => document.documentElement.style.setProperty('--test-color', 'rgb(1, 2, 3)'));
  assert.equal(await page.evaluate(() => getComputedStyle(document.documentElement).getPropertyValue('--test-color').trim()), 'rgb(1, 2, 3)');
  await page.setViewportSize({width: 760, height: 580});
  await rows.first().focus(); await page.keyboard.press('Shift+F10');
  const bounds = await page.locator('#context-menu').boundingBox();
  assert.ok(bounds.x >= 0 && bounds.y >= 0 && bounds.x + bounds.width <= 760 && bounds.y + bounds.height <= 580);
  await page.keyboard.press('Escape');
  await page.goto(`${origin}/download-detail.html`);
  await page.locator('#d-name').filter({hasText: 'Stories'}).waitFor();
  await page.locator('.extra-information summary').click();
  for (const id of ['d-checksum', 'd-actual-checksum', 'd-checksum-status']) assert.equal(await page.locator(`#${id}`).isVisible(), true);
  assert.deepEqual(errors, [], 'local UI scripts must load without runtime errors under CSP');
  console.log('PASS: table roles, scoped keys, menu focus/navigation, checksum fields, CSP and dynamic styles');
}

(async () => {
  const browser = await chromium.launch({executablePath: process.env.LDM_BROWSER || '/usr/bin/microsoft-edge-stable', headless: true});
  try {
    const context = await browser.newContext({viewport: {width: 1200, height: 820}});
    await context.route('**/*', async route => {
      const url = new URL(route.request().url());
      if (url.origin !== origin) return route.abort();
      if (url.pathname === '/capture-fixture.html' || url.pathname === '/embed/player') return route.fulfill({contentType: 'text/html', body: '<!doctype html><html><body></body></html>'});
      if (url.pathname === '/csp-probe.js') return route.fulfill({contentType: 'text/javascript', body: `
        window.cspProbe = {local: true, evalBlocked: false, inlineRan: false, remoteBlocked: false};
        document.addEventListener('securitypolicyviolation', event => { if (event.effectiveDirective.startsWith('script-src') && event.blockedURI.startsWith('https://invalid.example')) cspProbe.remoteBlocked = true; });
        try { new Function('return 1')(); } catch { cspProbe.evalBlocked = true; }
        const inline = document.createElement('script'); inline.textContent = 'cspProbe.inlineRan = true'; document.head.append(inline);
        const remote = document.createElement('script'); remote.src = 'https://invalid.example/forbidden.js'; document.head.append(remote);
      `});
      const file = path.basename(url.pathname);
      if (!/^[a-z-]+\.(html|js|css)$/.test(file) || !fs.existsSync(path.join(root, 'ui', file))) return route.fulfill({status: 404, body: ''});
      return route.fulfill({contentType: file.endsWith('.js') ? 'text/javascript' : file.endsWith('.css') ? 'text/css' : 'text/html', headers: {'Content-Security-Policy': csp}, body: read(`ui/${file}`)});
    });
    const page = await context.newPage();
    for (const kind of ['file','hover','quality','stream','generic','social','reddit','twitter','youtube','embed']) await captureControl(page, kind);
    await popupControls(page);
    await uiChecks(page);
    await context.close();
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exitCode = 1; });
