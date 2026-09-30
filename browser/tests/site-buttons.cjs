const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

// --- Test 1: YouTube single button & hoverButton suppressed ---
{
  let ytButton = null;
  const player = {
    contains: (el) => el === ytButton,
    querySelector: (sel) => sel.includes('ldm-yt-download-btn') ? ytButton : null,
    style: { setProperty() {} },
    appendChild(el) { ytButton = el; }
  };
  const video = {
    parentElement: player,
    getBoundingClientRect: () => ({ width: 1280, height: 720 })
  };
  const document = {
    getElementById: (id) => id === 'ldm-yt-download-btn' ? ytButton : null,
    querySelector: (sel) => {
      if (sel.includes('#movie_player')) return player;
      if (sel.includes('ldm-yt-download-btn')) return ytButton;
      return null;
    },
    querySelectorAll: (sel) => sel === 'video' ? [video] : [],
    documentElement: { appendChild() {} },
    createElement: (tag) => ({
      tagName: tag.toUpperCase(),
      setAttribute() {},
      addEventListener() {},
      style: { setProperty() {} },
      classList: { add() {}, remove() {} }
    }),
    addEventListener() {}
  };
  const window = {
    location: { href: 'https://www.youtube.com/watch?v=test1234', hostname: 'www.youtube.com', pathname: '/watch' },
    getComputedStyle: () => ({ position: 'relative' }),
    addEventListener() {}
  };

  const context = vm.createContext({ document, window, URL, console, setTimeout, clearTimeout, setInterval, clearInterval });
  const source = fs.readFileSync(path.join(__dirname, '../chromium/content-script.js'), 'utf8');
  vm.runInContext(source.replace('\nbootstrap();', '\n'), context);

  // Verify isYtdlpSupportedSite identifies YouTube
  assert.equal(context.isYtdlpSupportedSite(), true, 'YouTube must be recognized as ytdlp supported site');

  // Verify injectYouTubeButton injects the button
  context.injectYouTubeButton();
  assert.ok(ytButton, 'YouTube download button must be injected');
  assert.equal(ytButton.id, 'ldm-yt-download-btn', 'YouTube button must have id ldm-yt-download-btn');

  // Verify second run does not duplicate
  const firstButton = ytButton;
  context.injectYouTubeButton();
  assert.equal(ytButton, firstButton, 'YouTube button must not duplicate');

  let appendedToRoot = [];
  document.documentElement.appendChild = (el) => appendedToRoot.push(el);

  // Verify pointer over video does not pop up a hover button or append extra badges
  context.handlePointerOver({ clientX: 200, clientY: 200, target: video });
  assert.equal(appendedToRoot.length, 0, 'handlePointerOver must never append hoverButton on YouTube');

  console.log('YouTube single button and no duplicate/hover button checks passed.');
}

// --- Test 2: Facebook video container and button injection ---
{
  let fbButton = null;
  const videoWrapper = {
    matches: () => false,
    querySelector: (sel) => sel.includes('ldm-site-btn') ? fbButton : null,
    style: { setProperty() {} },
    appendChild(el) { fbButton = el; },
    getBoundingClientRect: () => ({ width: 640, height: 360 })
  };
  const postArticle = {
    matches: (sel) => sel.includes('role="article"') || sel.includes('article'),
    querySelector: (sel) => {
      if (sel.includes('/watch')) return { href: 'https://www.facebook.com/watch/?v=987654321' };
      return null;
    }
  };
  videoWrapper.parentElement = postArticle;

  const video = {
    parentElement: videoWrapper,
    closest: (sel) => {
      if (sel.includes('role="article"')) return postArticle;
      return null;
    },
    getBoundingClientRect: () => ({ width: 640, height: 360 })
  };

  const document = {
    querySelectorAll: (sel) => sel === 'video' ? [video] : [],
    documentElement: { appendChild() {} },
    createElement: (tag) => ({
      tagName: tag.toUpperCase(),
      setAttribute() {},
      addEventListener() {},
      style: { setProperty() {} },
      classList: { add() {}, remove() {} }
    }),
    addEventListener() {}
  };
  const window = {
    location: { href: 'https://www.facebook.com/', hostname: 'www.facebook.com', pathname: '/' },
    getComputedStyle: () => ({ position: 'relative' }),
    addEventListener() {}
  };

  const context = vm.createContext({ document, window, URL, console, setTimeout, clearTimeout, setInterval, clearInterval });
  const source = fs.readFileSync(path.join(__dirname, '../chromium/content-script.js'), 'utf8');
  vm.runInContext(source.replace('\nbootstrap();', '\n'), context);

  // Container lookup
  const container = context.findFacebookVideoContainer(video);
  assert.equal(container, videoWrapper, 'Should find videoWrapper as the tight container instead of whole article');

  // Inject Facebook buttons
  context.injectFacebookButtons();
  assert.ok(fbButton, 'Facebook download button should be injected');
  assert.equal(fbButton.className, 'ldm-site-btn', 'Facebook button should have ldm-site-btn class');

  // URL extraction
  const url = context.extractFacebookVideoUrl(video, container);
  assert.equal(url, 'https://www.facebook.com/watch/?v=987654321', 'Should extract video url from post article');

  // Test React unmounting recovery: if React removes button, next cycle re-injects
  fbButton = null; // simulate React re-render wiping inner HTML
  context.injectFacebookButtons();
  assert.ok(fbButton, 'Facebook button must be re-injected after React unmounts it');

  console.log('Facebook button injection, URL extraction, and React resilience checks passed.');
}
