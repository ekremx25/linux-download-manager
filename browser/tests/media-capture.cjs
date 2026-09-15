const fs = require('node:fs');
const vm = require('node:vm');
const assert = require('node:assert/strict');
const path = require('node:path');
const listeners = {};
const stub = (name = '') => new Proxy(() => {}, {
  get: (_, key) => key === 'addListener'
    ? fn => { listeners[name] = fn; }
    : stub(`${name}.${String(key)}`)
});
const context = vm.createContext({chrome: stub('chrome'), URL, console, setTimeout,
  self: {navigator: {userAgent: 'Test'}}});
vm.runInContext(fs.readFileSync(path.join(__dirname, '../chromium/service-worker.js'), 'utf8'), context);
const page = 'https://yabancidizi.news/dizi/example/sezon-1/bolum-1';
assert.equal(context.chooseBestMediaCapturePayload(1, null, page, '').ok, false);
assert.equal(context.canTryYtdlpPage('https://www.youtube.com/watch?v=test'), true);
const xPost = 'https://x.com/user/status/123';
let xCapture = context.chooseBestMediaCapturePayload(1, null, xPost, 'X video');
assert.equal(xCapture.capture.url, xPost);
assert.equal(xCapture.capture.forceYtdlp, true);
assert.equal(xCapture.capture.audioUrl, null);
assert.equal(context.isTwitterStatusPageUrl('https://x.com.evil.test/user/status/123'), false);
assert.equal(context.isTwitterStatusPageUrl('https://x.com/user'), false);
const response = (url, mime, statusCode = 200) => listeners['chrome.webRequest.onHeadersReceived']({
  tabId: 1, url, statusCode, initiator: 'https://player.example',
  responseHeaders: [{name: 'Content-Type', value: mime}]
});
response('https://video.twimg.com/ext_tw_video/123/pl/avc1/playlist.m3u8', 'application/vnd.apple.mpegurl');
xCapture = context.chooseBestMediaCapturePayload(1, null, xPost, 'X video');
assert.equal(xCapture.capture.url, xPost, 'captured video-only rendition must not replace the X post');
assert.equal(xCapture.capture.forceYtdlp, true);
listeners['chrome.tabs.onRemoved'](1);
response('https://cdn.example/opaque-token', 'application/vnd.apple.mpegurl; charset=utf-8');
let result = context.chooseBestMediaCapturePayload(1, null, page, 'Example');
assert.equal(result.capture.url, 'https://cdn.example/opaque-token');
assert.equal(result.capture.streamManifest, true);
context.rememberMediaRequest(1, result.capture.url, 'observed');
assert.equal(context.chooseBestMediaCapturePayload(1, null, page, '').capture.streamManifest, true);
response('https://cdn.example/segment', 'video/mp2t');
assert.equal(context.chooseBestMediaCapturePayload(1, null, page, '').capture.url, 'https://cdn.example/opaque-token');
listeners['chrome.tabs.onRemoved'](1);
response('https://cdn.example/error', 'application/vnd.apple.mpegurl', 403);
response('https://cdn.example/page', 'text/html');
assert.equal(context.chooseBestMediaCapturePayload(1, null, page, '').ok, false);
response('https://cdn.example/video/embed/asset', 'video/mp4');
assert.equal(context.chooseBestMediaCapturePayload(1, null, page, '').capture.url, 'https://cdn.example/video/embed/asset');
assert.equal(context.shouldRememberMediaRequest('https://cdn.example/asset', 'media'), true);
assert.equal(context.shouldRememberMediaRequest('https://cdn.example/asset', 'audio'), true);
assert.equal(context.shouldRememberMediaRequest('https://cdn.example/poster.jpg', 'media'), false);
console.log('Media capture regression checks passed.');
let saved;
context.chrome = {runtime: {getManifest: () => ({version: '0.1.21'})},
  downloads: {download: options => {saved = options;}}};
context.saveCaptureDiagnostic(1, 'https://example.test/page?token=SECRET');
const report = JSON.parse(decodeURIComponent(saved.url.split(',').slice(1).join(',')));
assert.equal(report.page.origin, 'https://example.test');
assert.equal(JSON.stringify(report).includes('SECRET'), false);
assert.equal(saved.filename, 'LDM-capture-diagnostic.json');
console.log('Diagnostic export and URL redaction checks passed.');
let failureReply;
context.chrome.downloads.download = () => {throw new Error('Export unavailable');};
context.console = {warn() {}};
listeners['chrome.runtime.onMessage']({type:'capture-best-media',payload:{sourcePageUrl:page}},
  {tab:{id:99,url:page}}, reply => {failureReply = reply;});
assert.equal(failureReply.ok, false);
assert.match(failureReply.error, /Video akışı/);
console.log('Failed diagnostic export still returns capture error.');
