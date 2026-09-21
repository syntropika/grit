(() => {
  "use strict";

  const minimumZoom = 0.001;
  const maximumZoom = 4;
  const clampZoom = (value) =>
    Math.max(minimumZoom, Math.min(maximumZoom, value));

  function fitTransform(bounds, size, padding = 48) {
    const zoom = clampZoom(
      Math.min(
        Math.max(1, size.width - padding * 2) / Math.max(1, bounds.width),
        Math.max(1, size.height - padding * 2) / Math.max(1, bounds.height),
        1.6,
      ),
    );
    return {
      zoom,
      x: (size.width - bounds.width * zoom) / 2 - bounds.x * zoom,
      y: (size.height - bounds.height * zoom) / 2 - bounds.y * zoom,
    };
  }

  function zoomTransform(state, value, anchor) {
    const zoom = clampZoom(value);
    const ratio = zoom / state.zoom;
    return {
      zoom,
      x: anchor.x - (anchor.x - state.x) * ratio,
      y: anchor.y - (anchor.y - state.y) * ratio,
    };
  }

  function create(canvas, viewport, onChange) {
    let bounds = { x: 0, y: 0, width: 520, height: 380 };
    let state = { zoom: 1, x: 0, y: 0 };
    const pointers = new Map();

    const size = () => ({
      width: canvas.clientWidth || 1,
      height: canvas.clientHeight || 1,
    });
    let renderedSize = size();
    let windowSize = { width: window.innerWidth, height: window.innerHeight };
    const center = () => ({ x: size().width / 2, y: size().height / 2 });
    const render = () => {
      const dimensions = size();
      renderedSize = dimensions;
      canvas.setAttribute(
        "viewBox",
        `0 0 ${dimensions.width} ${dimensions.height}`,
      );
      viewport.setAttribute(
        "transform",
        `translate(${state.x} ${state.y}) scale(${state.zoom})`,
      );
      canvas.dataset.zoom = String(state.zoom);
      canvas.dataset.panX = String(state.x);
      canvas.dataset.panY = String(state.y);
      onChange?.({ ...state });
    };
    const fit = () => {
      state = fitTransform(bounds, size());
      render();
    };
    const zoomTo = (value, anchor = center()) => {
      state = zoomTransform(state, value, anchor);
      render();
    };
    const panBy = (x, y) => {
      state = { ...state, x: state.x + x, y: state.y + y };
      render();
    };

    canvas.addEventListener(
      "wheel",
      (event) => {
        event.preventDefault();
        const rect = canvas.getBoundingClientRect();
        zoomTo(state.zoom * Math.exp(-event.deltaY * 0.002), {
          x: event.clientX - rect.left,
          y: event.clientY - rect.top,
        });
      },
      { passive: false },
    );
    const gesture = () => {
      const points = [...pointers.values()];
      if (points.length < 2) return { center: points[0], distance: 0 };
      return {
        center: {
          x: (points[0].x + points[1].x) / 2,
          y: (points[0].y + points[1].y) / 2,
        },
        distance: Math.hypot(
          points[0].x - points[1].x,
          points[0].y - points[1].y,
        ),
      };
    };
    canvas.addEventListener("pointerdown", (event) => {
      if (event.button !== 0 || event.target.closest(".graph-node")) return;
      pointers.set(event.pointerId, { x: event.clientX, y: event.clientY });
      canvas.dataset.dragging = "true";
      canvas.focus({ preventScroll: true });
      try {
        canvas.setPointerCapture(event.pointerId);
      } catch {
        /* Synthetic test events have no active pointer. */
      }
    });
    canvas.addEventListener("pointermove", (event) => {
      if (!pointers.has(event.pointerId)) return;
      const before = gesture();
      pointers.set(event.pointerId, { x: event.clientX, y: event.clientY });
      const after = gesture();
      if (before.distance > 0 && after.distance > 0) {
        const rect = canvas.getBoundingClientRect();
        state = zoomTransform(
          state,
          (state.zoom * after.distance) / before.distance,
          {
            x: before.center.x - rect.left,
            y: before.center.y - rect.top,
          },
        );
      }
      panBy(after.center.x - before.center.x, after.center.y - before.center.y);
    });
    const endDrag = (event) => {
      pointers.delete(event.pointerId);
      if (!pointers.size) delete canvas.dataset.dragging;
    };
    canvas.addEventListener("pointerup", endDrag);
    canvas.addEventListener("pointercancel", endDrag);
    canvas.addEventListener("lostpointercapture", endDrag);
    canvas.addEventListener("keydown", (event) => {
      const movement = {
        ArrowLeft: [48, 0],
        ArrowRight: [-48, 0],
        ArrowUp: [0, 48],
        ArrowDown: [0, -48],
      }[event.key];
      if (movement) {
        event.preventDefault();
        panBy(...movement);
      } else if (["+", "=", "-", "0"].includes(event.key)) {
        event.preventDefault();
        if (event.key === "0") fit();
        else zoomTo(state.zoom * (event.key === "-" ? 1 / 1.25 : 1.25));
      }
    });
    const resize = new ResizeObserver(() => {
      if (!canvas.clientWidth || !canvas.clientHeight) return;
      const dimensions = size();
      const nextWindow = {
        width: window.innerWidth,
        height: window.innerHeight,
      };
      if (
        nextWindow.width !== windowSize.width ||
        nextWindow.height !== windowSize.height
      ) {
        windowSize = nextWindow;
        fit();
      } else {
        // Opening an inspector preserves the explored scale and world center.
        state = {
          ...state,
          x: state.x + (dimensions.width - renderedSize.width) / 2,
          y: state.y + (dimensions.height - renderedSize.height) / 2,
        };
        render();
      }
    });
    resize.observe(canvas);

    return Object.freeze({
      setBounds: (value) => {
        bounds = { ...value };
      },
      fit,
      zoomTo,
      panBy,
      focus: (point) => {
        const anchor = center();
        state = {
          ...state,
          x: anchor.x - point.x * state.zoom,
          y: anchor.y - point.y * state.zoom,
        };
        render();
      },
      current: () => ({ ...state }),
    });
  }

  const api = Object.freeze({ create, fitTransform, zoomTransform });
  globalThis.HyfaGraphCamera = api;
  if (typeof module !== "undefined" && module.exports) module.exports = api;
})();
