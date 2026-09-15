const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

let button = null;
const container = {
  querySelector: () => button,
  style: { setProperty() {} },
  appendChild(element) { button = element; }
};
const video = {
  closest(selector) { return selector === 'article' ? null : container; },
  parentElement: container,
  getBoundingClientRect: () => ({ width: 640, height: 360 })
};
const document = {
  querySelectorAll: () => [video],
  createElement: () => ({
    setAttribute() {},
    addEventListener() {},
    style: { setProperty() {} }
  })
};
const window = {
  location: { href: 'https://x.com/user/status/123', pathname: '/user/status/123' },
  getComputedStyle: () => ({ position: 'static' })
};
const context = vm.createContext({ document, window, URL, console });
const source = fs.readFileSync(path.join(__dirname, '../chromium/content-script.js'), 'utf8');
vm.runInContext(source.replace('\nbootstrap();', '\n'), context);

context.injectTwitterButtons();
assert.equal(button?.className, 'ldm-site-btn', 'expanded X video should get a button without an article');
const firstButton = button;
context.injectTwitterButtons();
assert.equal(button, firstButton, 'periodic refresh should not add a duplicate button');
console.log('Expanded X video button checks passed.');
