const test = require("node:test");
const assert = require("node:assert/strict");
const { fitTransform, zoomTransform } = require("../src/graph/render/graph-camera.js");

const close = (actual, expected) => assert.ok(Math.abs(actual - expected) < 1e-9, `${actual} != ${expected}`);

test("fit centers a graph with negative coordinates inside desktop and mobile viewports", () => {
  const bounds = Object.freeze({ x: -340, y: 75, width: 1700, height: 960 });
  for (const size of [{ width: 1100, height: 740 }, { width: 390, height: 580 }]) {
    const state = fitTransform(bounds, size);
    assert.ok(bounds.x * state.zoom + state.x >= 48 - 1e-9);
    assert.ok(bounds.y * state.zoom + state.y >= 48 - 1e-9);
    assert.ok((bounds.x + bounds.width) * state.zoom + state.x <= size.width - 48 + 1e-9);
    assert.ok((bounds.y + bounds.height) * state.zoom + state.y <= size.height - 48 + 1e-9);
    close((bounds.x + bounds.width / 2) * state.zoom + state.x, size.width / 2);
    close((bounds.y + bounds.height / 2) * state.zoom + state.y, size.height / 2);
  }
});

test("anchored zoom preserves the exact world point beneath the cursor without changing source state", () => {
  const state = Object.freeze({ zoom: 0.35, x: -185, y: 143 });
  const anchor = Object.freeze({ x: 210, y: 330 });
  for (const value of [0.1, 0.7, 4, 200]) {
    const next = zoomTransform(state, value, anchor);
    close((anchor.x - next.x) / next.zoom, (anchor.x - state.x) / state.zoom);
    close((anchor.y - next.y) / next.zoom, (anchor.y - state.y) / state.zoom);
    assert.ok(next.zoom > 0 && next.zoom <= 4);
  }
});

test("empty and single-node graph bounds produce finite camera transforms", () => {
  for (const size of [{ width: 1, height: 1 }, { width: 390, height: 600 }]) {
    const state = fitTransform({ x: 0, y: 0, width: 0, height: 0 }, size);
    assert.ok(Object.values(state).every(Number.isFinite));
    assert.ok(state.zoom > 0);
  }
});

test("opening an inspector preserves zoom and the world center while device resizing refits", () => {
  const { create } = require("../src/graph/render/graph-camera.js");
  const previousWindow = global.window;
  const previousObserver = global.ResizeObserver;
  let resized;
  global.window = { innerWidth: 390, innerHeight: 844 };
  global.ResizeObserver = class { constructor(callback) { resized = callback; } observe() {} };
  const canvas = { clientWidth: 390, clientHeight: 560, dataset: {}, setAttribute() {}, addEventListener() {} };
  try {
    const camera = create(canvas, { setAttribute() {} });
    camera.setBounds({ x: 0, y: 0, width: 1700, height: 960 });
    camera.fit();
    camera.zoomTo(0.8);
    camera.focus({ x: 800, y: 450 });
    const before = camera.current();
    canvas.clientHeight = 240;
    resized();
    const after = camera.current();
    close(after.zoom, before.zoom);
    close((canvas.clientWidth / 2 - after.x) / after.zoom, 800);
    close((canvas.clientHeight / 2 - after.y) / after.zoom, 450);
    global.window.innerWidth = 844;
    canvas.clientWidth = 844;
    resized();
    assert.notEqual(camera.current().zoom, before.zoom);
  } finally {
    global.window = previousWindow;
    global.ResizeObserver = previousObserver;
  }
});
