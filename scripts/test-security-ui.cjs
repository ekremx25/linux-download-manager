// Run from the repository root: node scripts/test-security-ui.cjs
// VM event objects are harness controls, NOT evidence that page JavaScript can
// manufacture a trusted DOM event. Browser-native isTrusted still needs a real
// browser check. No browser installation, network, native host or profile is used.
const fs = require('node:fs');
const vm = require('node:vm');
const assert = require('node:assert/strict');
const tests = [];
const test = (name, run) => tests.push({name, run});
const read = path => fs.readFileSync(path, 'utf8');
const uiSource = path => read(path).replace(/^import .*;\n/, '');

async function extensionHarness(extensions = ['zip']) {
  const nativePayloads = [];
  let onMessage;
  const noop = () => {};
  const any = new Proxy(noop, {get: () => any});
  const settings = {enabled: true, extensions, excludedSites: []};
  const chrome = new Proxy({}, {get: (target, key) => target[key] || any});
  chrome.runtime = {
    onInstalled: {addListener: noop},
    onMessage: {addListener: fn => { onMessage = fn; }},
    getURL: path => `chrome-extension://test/${path}`,
    sendNativeMessage(host, payload, callback) {
      if (payload.action === 'capture_settings') {
        callback({ok: true, configured: true, settings});
      } else {
        nativePayloads.push(payload);
        callback({ok: true, id: 1});
      }
    }
  };
  chrome.storage = {local: {get: async () => ({captureSettings: settings}), set: async () => {}}, onChanged: {addListener: noop}};
  chrome.cookies = {getAll: async () => []};
  chrome.tabs = {get: async () => ({url: 'https://example.com/page'}), sendMessage: noop, onRemoved: {addListener: noop}, onUpdated: {addListener: noop}};
  const worker = {chrome, URL, console: {info: noop, warn: noop, error: noop}, setTimeout: noop, self: {navigator: {userAgent: 'test'}}};
  vm.createContext(worker);
  vm.runInContext(read('browser/chromium/service-worker.js'), worker);
  await vm.runInContext('captureReady', worker);
  const pending = [];
  const runtime = {id: 'test', sendMessage(message, callback) {
    pending.push(new Promise(resolve => {
      onMessage(message, {url: 'https://example.com/page', tab: {id: 1, url: 'https://example.com/page'}}, response => {
        callback(response);
        resolve();
      });
    }));
  }};
  const content = {URL, chrome: {runtime}, window: {location: {href: 'https://example.com/page', hostname: 'example.com'}}};
  vm.createContext(content);
  vm.runInContext(read('browser/chromium/content-script.js').replace('\nbootstrap();', ''), content);
  vm.runInContext('showToast=()=>{};', content);
  const flush = async () => { while (pending.length) await Promise.all(pending.splice(0)); };
  let replays = 0;
  const anchor = {
    tagName: 'A', href: 'https://example.com/archive.zip', getAttribute: () => '',
    click() { replays++; click(false); }
  };
  function click(trusted, modifiers = {}) {
    const event = {isTrusted: trusted, button: 0, defaultPrevented: false, composedPath: () => [anchor],
      preventDefault() { this.defaultPrevented = true; }, stopImmediatePropagation() {}, ...modifiers};
    content.event = event;
    vm.runInContext('handleFileLinkClick(event)', content);
    return event;
  }
  return {content, nativePayloads, click, flush, get replays() { return replays; }};
}

test('B02: synthetic file click sends zero native download payloads', async () => {
  const h = await extensionHarness();
  h.click(false);
  await h.flush();
  assert.equal(h.nativePayloads.length, 0);
});

test('B02 control: genuine unmodified file click sends exactly one native payload', async () => {
  const h = await extensionHarness();
  const event = h.click(true);
  await h.flush();
  assert.equal(event.defaultPrevented, true);
  assert.equal(h.nativePayloads.length, 1);
  assert.equal(h.nativePayloads[0].url, 'https://example.com/archive.zip');
  assert.equal(h.replays, 0);
  h.click(true, {ctrlKey: true});
  await h.flush();
  assert.equal(h.nativePayloads.length, 1, 'modified clicks must keep browser behavior');
});

test('B02 control: rejected capture replays once without native recursion or reusable trust', async () => {
  const h = await extensionHarness(['pdf']);
  h.click(true);
  await h.flush();
  assert.equal(h.nativePayloads.length, 0);
  assert.equal(h.replays, 1, 'unsupported type must fall back to anchor.click() exactly once');
  const synthetic = h.click(false);
  await h.flush();
  assert.equal(h.replays, 1, 'fallback must not authorize a later page-generated click');
  assert.equal(synthetic.defaultPrevented, false, 'untrusted click should not be intercepted');
});

function mainHarness() {
  const context = {document: {addEventListener() {}}, isTauri: true, setNotice() {}};
  vm.createContext(context);
  vm.runInContext(uiSource('ui/main.js'), context);
  vm.runInContext('render=()=>{};', context);
  return context;
}

test('B07: queued DB state after Resume replaces a paused live overlay', async () => {
  const context = mainHarness();
  context.invoke = async () => [{id: 1, status: 'queued', downloadedBytes: 12}];
  vm.runInContext("state.downloads=[{id:1,status:'paused'}];state.live.set(1,{id:1,status:'paused',downloadedBytes:12});", context);
  await vm.runInContext('loadDownloads()', context);
  assert.equal(vm.runInContext('merged(state.downloads[0]).status', context), 'queued');
});

test('B07 control: queued DB polling does not erase newer receiving progress', async () => {
  const context = mainHarness();
  context.invoke = async () => [{id: 1, status: 'queued', downloadedBytes: 12}];
  vm.runInContext("state.downloads=[{id:1,status:'queued'}];state.live.set(1,{id:1,status:'in_progress',downloadedBytes:24});", context);
  await vm.runInContext('loadDownloads()', context);
  assert.equal(vm.runInContext('merged(state.downloads[0]).status', context), 'in_progress');
  assert.equal(vm.runInContext('merged(state.downloads[0]).downloadedBytes', context), 24);
});

function detailHarness() {
  const elements = new Map();
  const context = {
    document: {addEventListener() {}, getElementById(id) {
      if (!elements.has(id)) elements.set(id, {textContent: '', style: {}, parentElement: {setAttribute() {}}});
      return elements.get(id);
    }},
    window: {addEventListener() {}}, progressOf: () => 0,
    statusLabel: value => value, formatBytes: String, formatSpeed: String,
    formatTime: String, escapeHtml: String, setNotice() {}
  };
  vm.createContext(context);
  vm.runInContext(uiSource('ui/download-detail.js'), context);
  return {context, elements};
}

test('B07: detail reload reconciles paused overlay with queued DB state', async () => {
  const {context, elements} = detailHarness();
  vm.runInContext("selectedId=1;current={download:{id:1,status:'paused'}};live={id:1,status:'paused'};", context);
  context.invoke = async () => ({download: {id: 1, fileName: 'test.zip', status: 'queued'}, segments: []});
  await vm.runInContext('load(1)', context);
  assert.equal(elements.get('d-status').textContent, 'queued');
  assert.equal(elements.get('pause-resume').textContent, 'Pause');
});

test('B05: detail exposes expected checksum, actual checksum and mismatch status', () => {
  const {context, elements} = detailHarness();
  context.detail = {download: {id: 1, fileName: 'test.zip', status: 'failed', expectedChecksum: 'a'.repeat(64), actualChecksum: 'b'.repeat(64), checksumStatus: 'mismatch'}, segments: []};
  vm.runInContext('render(detail)', context);
  const text = [...elements.values()].map(element => element.textContent).join('\n');
  assert.ok(text.includes('a'.repeat(64)), 'expected SHA-256 must be visible');
  assert.ok(text.includes('b'.repeat(64)), 'actual SHA-256 must be visible');
  assert.match(text, /mismatch/i, 'failure must be explicit, not just an expected checksum');
});

test('B07: stale polling cannot overwrite a newer paused event', async () => {
  const context = mainHarness();
  let release;
  context.invoke = () => new Promise(resolve => { release = resolve; });
  vm.runInContext("state.downloads=[{id:1,status:'queued'}];state.live.set(1,{id:1,status:'in_progress'});", context);
  const polling = vm.runInContext('loadDownloads()', context);
  vm.runInContext("state.live.set(1,{id:1,status:'paused',downloadedBytes:24});", context);
  release([{id:1,status:'queued',downloadedBytes:12}]);
  await polling;
  assert.equal(vm.runInContext('merged(state.downloads[0]).status', context), 'paused');
  assert.equal(vm.runInContext('merged(state.downloads[0]).downloadedBytes', context), 24);
});

test('B07: stale detail polling preserves a newer paused event', async () => {
  const {context, elements} = detailHarness();
  let release;
  context.invoke = () => new Promise(resolve => { release = resolve; });
  vm.runInContext("selectedId=1;current={download:{id:1,status:'queued'}};live={id:1,status:'in_progress'};", context);
  const polling = vm.runInContext('load(1)', context);
  vm.runInContext("live={id:1,status:'paused',downloadedBytes:24};", context);
  release({download:{id:1,fileName:'test.zip',status:'queued'},segments:[]});
  await polling;
  assert.equal(elements.get('d-status').textContent, 'paused');
});

test('CSP: local scripts only, IPC preserved, no eval, dynamic style attributes allowed', () => {
  const csp = JSON.parse(read('src-tauri/tauri.conf.json')).app.security.csp;
  assert.ok(csp, 'Tauri CSP must not be disabled');
  const directives = typeof csp === 'string'
    ? Object.fromEntries(csp.split(';').map(part => part.trim().split(/\s+/)).filter(([name]) => name).map(([name, ...values]) => [name, values]))
    : Object.fromEntries(Object.entries(csp).map(([name, values]) => [name, Array.isArray(values) ? values : values.split(/\s+/)]));
  const script = directives['script-src'] || directives['default-src'];
  assert.ok(script.includes("'self'"));
  assert.ok(!script.some(value => /unsafe-inline|unsafe-eval|^https?:|\*/.test(value)), 'scripts must not permit inline code, eval or remote origins');
  assert.deepEqual(directives['object-src'], ["'none'"]);
  const connect = directives['connect-src'] || directives['default-src'];
  for (const required of ["'self'", 'ipc:', 'http://ipc.localhost']) assert.ok(connect.includes(required), `missing IPC connect source ${required}`);
  const attributes = directives['style-src-attr'] || directives['style-src'] || directives['default-src'];
  assert.ok(attributes.includes("'unsafe-inline'"), 'progress widths and GTK theme variables require dynamic CSS');
});

test('B07: reconciliation also runs when the DB snapshot is unchanged', async () => {
  const context = mainHarness();
  context.invoke = async () => [{id: 1, status: 'queued', downloadedBytes: 12}];
  await vm.runInContext('loadDownloads()', context);
  vm.runInContext("state.live.set(1,{id:1,status:'paused',downloadedBytes:12});", context);
  await vm.runInContext('loadDownloads()', context);
  assert.equal(vm.runInContext('merged(state.downloads[0]).status', context), 'queued');
});

test('B07 control: detail retains newer receiving progress over queued polling', async () => {
  const {context, elements} = detailHarness();
  vm.runInContext("selectedId=1;current={download:{id:1,status:'queued'}};live={id:1,status:'in_progress',downloadedBytes:24};", context);
  context.invoke = async () => ({download: {id: 1, fileName: 'test.zip', status: 'queued', downloadedBytes: 12}, segments: []});
  await vm.runInContext('load(1)', context);
  assert.equal(elements.get('d-status').textContent, 'in_progress');
  assert.equal(elements.get('d-downloaded').textContent, '24');
});

test('B05 control: missing checksum metadata is explicitly unverified', () => {
  const {context, elements} = detailHarness();
  context.detail = {download: {id: 1, fileName: 'test.zip', status: 'completed'}, segments: []};
  vm.runInContext('render(detail)', context);
  assert.equal(elements.get('d-checksum').textContent, 'Not configured');
  assert.equal(elements.get('d-actual-checksum').textContent, 'Not available');
  assert.equal(elements.get('d-checksum-status').textContent, 'Not verified');
});

(async () => {
  let failed = 0;
  for (const {name, run} of tests) {
    try { await run(); console.log(`PASS: ${name}`); }
    catch (error) { failed++; console.error(`FAIL: ${name}\n${error.stack}`); }
  }
  console.log(`${tests.length - failed}/${tests.length} security/UI regressions passed`);
  if (failed) process.exitCode = 1;
})().catch(error => { console.error(error); process.exitCode = 1; });
