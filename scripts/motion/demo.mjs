// Plays the motion demo of the website: a statement typed into the editor,
// which outlines it as it grows; its result; a cell of that result edited in
// place; and a JSON cell opened in the expanded editor. Every state shown is a keyframe Blanco exported (the `motion`
// scene in crates/blanco/src/screenshots.rs). What moves between them is
// drawn here: the camera, the typing, the caret, the pointer and the keys.
// It plays once and stops on the whole window, with hotspots over features
// that tell more about them when pointed at.
//
//   import { mountDemo } from "./demo.mjs";
//   const bundle = await (await fetch("demo.json")).json();
//   const demo = mountDemo(element, bundle, { docs: "docs/" });
//
// `docs` is where the hotspots' "Read more" links point, the mdBook's root.
//
// A frame is a function of time alone, so `demo.seek(t)` shows exactly what
// playback shows at `t` seconds, as a video capture needs.

import { renderSvg } from "./render.mjs";

const WIDTH = 1640;
const HEIGHT = 984;

/// What the scene types, line by line, as the keyframes `02-line-1` to
/// `05-line-4` hold it.
const TYPED = [
  "SELECT id, title, format, price, stock, details",
  "FROM albums",
  "WHERE stock < 3",
  "ORDER BY stock, title;",
];
const EDIT_VALUE = "25";

const CARET = "#f5e0dc";

/// Keyframes captured after the demo, only for the hotspots' cards.
const MEDIA_KEYS = new Set(["14-context-menu", "15-copy-as", "16-script"]);

/// The hotspots shown once the demo stops: what each says, the docs page it
/// links to, and how to find where it goes in the last keyframe.
const HOTSPOTS = [
  {
    title: "Run what is framed",
    text: "The statement under the cursor is framed, and that is what runs. With text selected, the selection runs instead.",
    page: "running-queries.html",
    find: (items) => anchorAfter(textRun(items, (r) => r.text.includes("SELECT id, title"))),
  },
  {
    title: "Editor",
    text: "Schema aware completion, formatting and linting for SQL.",
    page: "editor.html",
    find: (items) => anchorOn(textRun(items, (r) => r.text === "Format" && r.y < 560)),
  },
  {
    title: "Schema",
    text: "Tables show their size in the sidebar, which has a filter. Open a table's structure, its DDL or an ER diagram from there.",
    page: "workspace.html",
    find: (items) => anchorOn(textRun(items, (r) => /KB$/.test(r.text) && r.x[0] < 340)),
  },
  {
    title: "Environments",
    text: "Tag connections LOCAL, DEV, TEST, STAGE or PROD. Writes to PROD ask first, and a connection can be read only.",
    page: "safety.html",
    find: (items) => anchorOn(textRun(items, (r) => r.text === "DEV" && r.x[0] < 340)),
  },
  {
    title: "Results",
    text: "Filter rows, copy or export as CSV, TSV, JSON, SQL or Markdown, and show results as a chart.",
    page: "results.html",
    find: (items) => anchorOn(textRun(items, (r) => r.text === "Filter rows")),
  },
  {
    title: "Cell editor",
    text: "Open any cell in a larger editor, and drag its corner to make it bigger. JSON values come formatted, with Format and Minify.",
    page: "editing-data.html",
    find: (items) => anchorOn(textRun(items, (r) => r.text.includes("Minify"))),
  },
  {
    title: "Copy and export",
    text: "Right click a cell to copy or export the result as CSV, TSV, JSON, SQL, VALUES, Markdown or HTML, or to copy a column's values.",
    page: "results.html",
    find: (items) => anchorOn(textRun(items, (r) => r.text === "Blue Room")),
    // The context menu with Copy as open, from the keyframe after the demo.
    media: (keyframes) => {
      const items = keyframes["15-copy-as"].items;
      const box = union(
        ["Copy as", "HTML", "Markdown", "VALUES", "Filter by Value", "Blue Room"].map((label) =>
          runBox(textRun(items, (r) => r.text === label)),
        ),
      );
      return { key: "15-copy-as", box: pad(box, 12) };
    },
  },
  {
    title: "Scripts",
    text: "Script tabs run JavaScript against the connection: feed one query into the next, loop, or reshape rows and show them in the grid.",
    page: "scripting.html",
    find: (items) => anchorOn(textRun(items, (r) => r.text === "Restock report")),
    // The top of the script, from the keyframe after the demo.
    media: (keyframes) => {
      const items = keyframes["16-script"].items;
      const first = textRun(items, (r) => r.text.includes("// Albums that sell out"));
      const last = textRun(items, (r) => r.text.includes("GROUP BY al.id"));
      // As wide as the longest of those lines.
      const lines = items.filter((r) => r.t === "text" && r.y >= first.y && r.y <= last.y && r.x[0] >= first.x[0] - 1);
      const right = Math.max(...lines.map((r) => r.x[r.x.length - 1] + r.size * 0.62));
      const box = [first.x[0], first.y - first.size, right - first.x[0], last.y - first.y + first.size * 1.4];
      return { key: "16-script", box: pad(box, 10) };
    },
  },
  {
    title: "Pending edits",
    text: "Edited cells wait as pending changes. Apply edits shows the SQL that will run, and runs it all in one transaction.",
    page: "editing-data.html#applying-changes",
    find: (items) => anchorOn(textRun(items, (r) => r.text === "Apply edits")),
  },
];

export function mountDemo(root, bundle, options = {}) {
  const { keyframes, icons } = bundle;
  const reduced =
    options.reducedMotion ??
    globalThis.matchMedia?.("(prefers-reduced-motion: reduce)").matches;

  injectStyles();
  root.classList.add("bd-root");
  root.innerHTML = `
    <div class="bd-viewport">
      <div class="bd-stage"><div class="bd-camera"></div></div>
      <div class="bd-overlay">
        <div class="bd-caret"></div>
        <div class="bd-ring"></div>
        <div class="bd-ripple"></div>
        <svg class="bd-pointer" viewBox="0 0 16 22" aria-hidden="true">
          <path d="M1 1v17l4.5-4.2 3 6.7 3-1.4-3-6.6H15z" fill="#fff" stroke="#11111b" stroke-width="1.4" stroke-linejoin="round"/>
        </svg>
        <div class="bd-keys"></div>
        <div class="bd-caption"></div>
        <div class="bd-dim"></div>
        <button class="bd-replay" type="button">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8"/><path d="M3 3v5h5"/></svg>
          Replay
        </button>
      </div>
    </div>
    <div class="bd-spots"></div>`;
  const viewport = root.querySelector(".bd-viewport");
  const stage = root.querySelector(".bd-stage");
  const camera = root.querySelector(".bd-camera");
  const caret = root.querySelector(".bd-caret");
  const ring = root.querySelector(".bd-ring");
  const ripple = root.querySelector(".bd-ripple");
  const pointer = root.querySelector(".bd-pointer");
  const keys = root.querySelector(".bd-keys");
  const caption = root.querySelector(".bd-caption");
  const dim = root.querySelector(".bd-dim");
  const spots = root.querySelector(".bd-spots");
  const replay = root.querySelector(".bd-replay");

  const layers = {};
  const page = Math.random().toString(36).slice(2, 6);
  const timeline = Object.entries(keyframes).filter(([name]) => !MEDIA_KEYS.has(name));
  for (const [ix, [name, snapshot]] of timeline.entries()) {
    const layer = document.createElement("div");
    layer.className = "bd-layer";
    // Every layer is on the same page, so each needs ids of its own.
    const idPrefix = `bd${page}k${ix}-`;
    layer.innerHTML = renderSvg(snapshot, { icons, fonts: options.fonts, idPrefix });
    camera.append(layer);
    layers[name] = layer;
  }

  // The statement's frame of each typing keyframe, on its own above the
  // layer: the frame hugs the text, so the part of a line still to be typed
  // would otherwise hide its border.
  const frames = {};
  for (const [ix, line] of TYPED.entries()) {
    const name = `0${ix + 2}-line-${ix + 1}`;
    const snapshot = keyframes[name];
    const items = snapshot.items.filter((item) => item.t === "path" && inEditor(item.d));
    const layer = document.createElement("div");
    layer.className = "bd-layer";
    layer.style.zIndex = 3;
    layer.innerHTML = renderSvg({ ...snapshot, items, traffic_lights: null }, {
      icons,
      idPrefix: `bd${page}f${ix}-`,
    });
    camera.append(layer);
    frames[name] = layer;
  }

  const plan = makePlan(keyframes);
  const rows = rowElements(layers["06-results"], keyframes["06-results"], plan.grid);
  buildHotspots(spots, dim, keyframes, plan.lastKey, {
    docs: options.docs ?? "docs/",
    scale: () => scale,
    // A card's crop of a keyframe is an SVG of its own, with ids of its own.
    media: (key, box, ix) =>
      renderSvg(keyframes[key], {
        icons,
        fonts: options.fonts,
        viewBox: box,
        idPrefix: `bd${page}m${ix}-`,
      }),
  });

  let scale = 1;
  const resize = () => {
    scale = viewport.clientWidth / WIDTH;
    stage.style.transform = `scale(${scale})`;
  };
  new ResizeObserver(resize).observe(viewport);
  resize();

  let shownCaption = null;
  let shownKeys = null;
  let staggered = false;

  function frame(t) {
    if (reduced) t = plan.duration;
    t = Math.min(t, plan.duration);
    // Stopped on the whole window, the hotspots take over.
    const ended = t >= plan.duration;
    root.classList.toggle("bd-ended", ended);

    // The camera, as the point of the stage at the centre of the view and a
    // zoom.
    const view = interpolate(plan.camera, t);
    const zoom = view.z;
    const tx = clamp(WIDTH / 2 - view.x * zoom, WIDTH - WIDTH * zoom, 0);
    const ty = clamp(HEIGHT / 2 - view.y * zoom, HEIGHT - HEIGHT * zoom, 0);
    camera.style.transform = `translate(${tx}px, ${ty}px) scale(${zoom})`;
    const toScreen = (x, y) => [(x * zoom + tx) * scale, (y * zoom + ty) * scale];

    // Keyframes: the one shown, and the one under it while it fades in or
    // is revealed character by character.
    const { base, top, opacity, reveal } = layerState(plan, t);
    for (const [name, layer] of Object.entries(layers)) {
      const visible = name === base || name === top;
      layer.style.display = visible ? "" : "none";
      layer.style.zIndex = name === top ? 2 : 1;
      layer.style.opacity = name === top ? opacity : 1;
      layer.style.clipPath = name === top && reveal ? notch(reveal) : "";
    }
    for (const [name, layer] of Object.entries(frames)) {
      layer.style.display = name === top && reveal ? "" : "none";
    }

    // Result rows come in one after another.
    const stagger = t - plan.results;
    if (base === "06-results" || top === "06-results") {
      rows.forEach((elements, row) => {
        const p = easeOut(clamp((stagger - 0.08 - row * 0.045) / 0.32, 0, 1));
        for (const element of elements) {
          element.style.opacity = p;
          element.style.transform = `translateY(${(1 - p) * 8}px)`;
        }
      });
      staggered = true;
    } else if (staggered) {
      rows.flat().forEach((element) => {
        element.style.opacity = "";
        element.style.transform = "";
      });
      staggered = false;
    }

    // The caret, solid while typing and blinking while it waits.
    const caretAt = caretState(plan, t);
    if (caretAt) {
      const [x, y] = toScreen(caretAt.x, caretAt.top);
      caret.style.display = "";
      caret.style.transform = `translate(${x}px, ${y}px)`;
      caret.style.height = `${caretAt.height * zoom * scale}px`;
      caret.style.width = `${Math.max(1.5, 1.6 * zoom * scale)}px`;
      caret.style.opacity = caretAt.blink ? (Math.floor(t * 1.8) % 2 ? 0 : 1) : 1;
    } else {
      caret.style.display = "none";
    }

    // The pointer, and the ripples of its clicks.
    const hand = interpolate(plan.pointer, t);
    if (hand.o > 0.01) {
      const [x, y] = toScreen(hand.x, hand.y);
      pointer.style.display = "";
      pointer.style.opacity = hand.o;
      pointer.style.transform = `translate(${x - 2}px, ${y - 1}px)`;
    } else {
      pointer.style.display = "none";
    }
    const click = plan.clicks.findLast((c) => t >= c.t && t < c.t + 0.45);
    if (click) {
      const p = (t - click.t) / 0.45;
      const [x, y] = toScreen(click.x, click.y);
      const size = 10 + 34 * easeOut(p);
      ripple.style.display = "";
      ripple.style.opacity = 0.55 * (1 - p);
      ripple.style.width = ripple.style.height = `${size}px`;
      ripple.style.transform = `translate(${x - size / 2}px, ${y - size / 2}px)`;
    } else {
      ripple.style.display = "none";
    }

    // A ring drawn around what the scene points at.
    const highlight = plan.rings.find((r) => t >= r.from && t < r.to);
    if (highlight) {
      // Kept inside the view, which a button at the window's edge would
      // otherwise push the ring out of.
      const [x0, y0] = toScreen(highlight.b[0] - 4, highlight.b[1] - 4).map((v) => Math.max(v, 3));
      const [x1, y1] = toScreen(highlight.b[0] + highlight.b[2] + 4, highlight.b[1] + highlight.b[3] + 4).map(
        (v, ix) => Math.min(v, (ix ? HEIGHT : WIDTH) * scale - 3),
      );
      const p = clamp(Math.min(t - highlight.from, highlight.to - t) / 0.3, 0, 1);
      ring.style.display = "";
      ring.style.opacity = p;
      ring.style.transform = `translate(${x0}px, ${y0}px)`;
      ring.style.width = `${x1 - x0}px`;
      ring.style.height = `${y1 - y0}px`;
    } else {
      ring.style.display = "none";
    }

    // Keys pressed, shown as keycaps.
    const press = plan.keys.find((k) => t >= k.from && t < k.to);
    if (press !== shownKeys) {
      keys.innerHTML = press ? press.keys.map((key) => `<kbd>${key}</kbd>`).join("") : "";
      shownKeys = press;
    }
    if (press) {
      const p = clamp(Math.min(t - press.from, press.to - t) / 0.18, 0, 1);
      keys.style.opacity = p;
      keys.classList.toggle("bd-pressed", t >= press.down && t < press.down + 0.18);
      keys.style.transform = `translate(-50%, ${(1 - p) * 10}px)`;
    }

    const words = ended
      ? { text: "Point at a dot to see more", from: -Infinity, to: Infinity }
      : plan.captions.find((c) => t >= c.from && t < c.to);
    if (words !== shownCaption) {
      caption.textContent = words ? words.text : "";
      shownCaption = words;
    }
    caption.style.opacity = ended
      ? ""
      : words
        ? clamp(Math.min(t - words.from, words.to - t) / 0.35, 0, 1)
        : 0;
  }

  let start = null;
  let offset = 0;
  let playing = false;
  let raf = 0;
  const tick = (now) => {
    if (start === null) start = now - offset * 1000;
    const t = (now - start) / 1000;
    offset = Math.min(t, plan.duration);
    frame(offset);
    if (t >= plan.duration) {
      playing = false;
      return;
    }
    raf = requestAnimationFrame(tick);
  };
  const play = () => {
    if (playing || reduced) return;
    if (offset >= plan.duration) offset = 0;
    playing = true;
    start = null;
    raf = requestAnimationFrame(tick);
  };
  replay.addEventListener("click", () => {
    pause();
    offset = 0;
    play();
  });
  const pause = () => {
    playing = false;
    cancelAnimationFrame(raf);
  };

  frame(0);
  if (options.autoplay !== false) {
    // Only play while on screen.
    // Only play while on screen, and only once: scrolled back to, it stays
    // stopped on the hotspots.
    new IntersectionObserver(
      ([entry]) => {
        if (!entry.isIntersecting) pause();
        else if (offset < plan.duration) play();
      },
      { threshold: 0.2 },
    ).observe(viewport);
  }

  return {
    duration: plan.duration,
    get ended() {
      return offset >= plan.duration;
    },
    play,
    pause,
    seek(t) {
      pause();
      offset = t;
      frame(t);
    },
  };
}

/// Adds the hotspots, placed by where their features are in `items`, the last
/// keyframe's. Pointing at one, or focusing or tapping it, shows its card and
/// dims the rest of the window around it.
function buildHotspots(container, dim, keyframes, lastKey, { docs, scale, media }) {
  const items = keyframes[lastKey].items;
  const base = docs.endsWith("/") ? docs : `${docs}/`;
  let index = 0;
  for (const spot of HOTSPOTS) {
    let at;
    try {
      at = spot.find(items);
    } catch {
      continue;
    }
    let figure = "";
    if (spot.media) {
      try {
        const { key, box } = spot.media(keyframes);
        figure = `<div class="bd-media" style="aspect-ratio: ${box[2]} / ${box[3]}">${media(key, box, index)}</div>`;
      } catch {
        // Without the keyframe the card goes without its picture.
      }
    }
    const id = `bd-card-${Math.random().toString(36).slice(2, 8)}`;
    const element = document.createElement("div");
    element.className = "bd-spot";
    element.style.left = `${(at.x / WIDTH) * 100}%`;
    element.style.top = `${(at.y / HEIGHT) * 100}%`;
    element.style.setProperty("--i", index++);
    element.dataset.side = at.x > WIDTH * 0.55 ? "left" : "right";
    element.dataset.rise = at.y > HEIGHT * 0.55 ? "up" : "down";
    element.innerHTML = `
      <button class="bd-dot" type="button" aria-label="${spot.title}" aria-describedby="${id}"></button>
      <div class="bd-card${figure ? " bd-wide" : ""}" role="tooltip" id="${id}">
        ${figure}
        <strong>${spot.title}</strong>
        <p>${spot.text}</p>
        <a href="${base}${spot.page}">Read more</a>
      </div>`;
    const show = () => {
      for (const other of container.querySelectorAll(".bd-open")) {
        if (other !== element) other.classList.remove("bd-open");
      }
      element.classList.add("bd-open");
      const radius = 70 * scale();
      dim.style.background = `radial-gradient(circle at ${(at.x / WIDTH) * 100}% ${(at.y / HEIGHT) * 100}%, transparent ${radius}px, rgb(17 17 27 / 0.62) ${radius + 70 * scale()}px)`;
      dim.classList.add("bd-on");
    };
    const hide = () => {
      element.classList.remove("bd-open");
      if (!container.querySelector(".bd-open")) dim.classList.remove("bd-on");
    };
    element.addEventListener("pointerenter", show);
    element.addEventListener("pointerleave", hide);
    element.addEventListener("focusin", show);
    element.addEventListener("focusout", (event) => {
      if (!element.contains(event.relatedTarget)) hide();
    });
    // A tap opens a card, a second tap closes it.
    element.querySelector(".bd-dot").addEventListener("click", () =>
      element.classList.contains("bd-open") ? hide() : show(),
    );
    container.append(element);
  }
}

function textRun(items, test) {
  const run = items.find((item) => item.t === "text" && test(item));
  if (!run) throw new Error("no such text");
  return run;
}

/// The box a text run fills: its clip when that is a button or badge around
/// it, otherwise the text itself.
function runBox(run) {
  const right = run.x[run.x.length - 1] + run.size * 0.6;
  if (run.clip && run.clip[2] < 220) return run.clip;
  return [run.x[0], run.y - run.size, right - run.x[0], run.size * 1.3];
}

/// A hotspot just off the top right corner of what a run labels, as a badge
/// sits, clear of the label itself.
function anchorOn(run) {
  const [x, y, w] = runBox(run);
  return { x: x + w + 3, y: y - 1 };
}

function union(boxes) {
  const left = Math.min(...boxes.map((b) => b[0]));
  const top = Math.min(...boxes.map((b) => b[1]));
  const right = Math.max(...boxes.map((b) => b[0] + b[2]));
  const bottom = Math.max(...boxes.map((b) => b[1] + b[3]));
  return [left, top, right - left, bottom - top];
}

function pad([x, y, w, h], by) {
  return [x - by, y - by, w + by * 2, h + by * 2];
}

/// A hotspot just after the end of a run's text, on its line.
function anchorAfter(run) {
  return { x: run.x[run.x.length - 1] + run.size * 1.8, y: run.y - run.size * 0.35 };
}

/// Works out the timeline from where things are in the keyframes.
function makePlan(keyframes) {
  // Typing: each line of the statement, and when each character appears.
  const lines = TYPED.map((text, ix) => {
    const key = `0${ix + 2}-line-${ix + 1}`;
    const run = findRun(keyframes[key], (r) => r.text.includes(text));
    const first = [...run.text].length - [...run.text.slice(run.text.indexOf(text))].length;
    return { key, text, run, first };
  });
  const typingStart = 1.9;
  let at = typingStart;
  let seed = 7;
  const random = () => ((seed = (seed * 16807) % 2147483647) / 2147483647);
  for (const line of lines) {
    line.start = at;
    line.chars = [...line.text].map((ch, ix) => {
      at += ch === " " ? 0.03 : 0.034 + random() * 0.03;
      return { t: at, x: line.run.x[line.first + ix] };
    });
    line.end = at + 0.05;
    line.top = line.run.y - line.run.size * 1.05;
    line.bottom = line.run.y + line.run.size * 0.4;
    // What is still to be typed: the glyphs of the line and no more, so the
    // frame drawn around the statement keeps its borders while it grows.
    line.hidden = {
      top: line.run.y - line.run.size * 0.95,
      bottom: line.run.y + line.run.size * 0.3,
      right: line.chars[line.chars.length - 1].x + line.run.size * 0.7,
    };
    // After the line, the caret goes to the next one.
    at += 0.28;
  }
  const typed = lines[lines.length - 1].end;

  const run = findRun(keyframes["05-line-4"], (r) => r.text === "Run Current");
  const results = typed + 1.0;

  const value = findRun(keyframes["09-typed"], (r) => r.text === EDIT_VALUE);
  const cell = value.clip; // The inline input, which covers the cell.
  const cellCenter = [cell[0] + cell[2] / 2, cell[1] + cell[3] / 2];
  const firstRow = findRun(keyframes["06-results"], (r) => r.text === "After Hours");
  const secondRow = findRun(
    keyframes["06-results"],
    (r) => r.text === "After Hours" && r.y > firstRow.y,
  );
  const grid = {
    top: firstRow.y - 13.5,
    rowHeight: secondRow.y - firstRow.y,
    left: firstRow.clip[0] - 200,
    bottom: firstRow.clip[1] + 300,
  };
  const apply = findRun(keyframes["10-edited"], (r) => r.text === "Apply edits");
  const applyButton = apply.clip;

  const select = results + 2.3;
  const open = select + 0.55;
  const typeValue = open + 0.55;
  const valueChars = [...EDIT_VALUE].map((_, ix) => ({
    t: typeValue + ix * 0.14,
    x: value.x[ix],
  }));
  const commit = typeValue + 0.75;

  // The first row's details cell, the one the scene opens. Runs come in
  // painting order, so the topmost is picked rather than the first found.
  const json = keyframes["06-results"].items
    .filter((item) => item.t === "text" && item.text.startsWith('{"catalog"'))
    .reduce((top, run) => (run.y < top.y ? run : top));
  const jsonPoint = [json.x[0] + 70, json.y - 4];
  const maximize = keyframes["12-json-inline"].items.find(
    (item) => item.t === "icon" && item.path.includes("maximize"),
  );
  const maximizePoint = maximize
    ? [maximize.b[0] + maximize.b[2] / 2, maximize.b[1] + maximize.b[3] / 2]
    : [jsonPoint[0] + 300, jsonPoint[1]];
  const jsonDocument = findRun(keyframes["13-json-expanded"], (r) => r.text.includes('"catalog": '));
  const editor = jsonDocument.clip;
  const editorCenter = [editor[0] + editor[2] / 2, editor[1] + editor[3] / 2 + 20];

  const jsonSelect = commit + 1.4;
  const jsonOpen = jsonSelect + 0.55;
  const jsonExpand = jsonOpen + 0.8;
  const toApply = jsonExpand + 2.4;
  const outro = toApply + 2.2;
  const duration = outro + 1.4;

  const statement = {
    x: (lines[0].chars[0].x + lines[0].chars[lines[0].chars.length - 1].x) / 2 + 40,
    y: (lines[0].top + lines[3].bottom) / 2 - 40,
  };

  return {
    duration,
    lastKey: "13-json-expanded",
    results,
    grid,
    lines,
    typingStart,
    typed,
    select,
    open,
    commit,
    jsonSelect,
    jsonOpen,
    jsonExpand,
    outro,
    value: {
      chars: valueChars,
      top: cell[1],
      bottom: cell[1] + cell[3],
      right: valueChars[valueChars.length - 1].x + value.size * 0.7,
    },
    camera: [
      { t: 0, x: WIDTH / 2, y: HEIGHT / 2, z: 1 },
      { t: 0.7, x: WIDTH / 2, y: HEIGHT / 2, z: 1 },
      { t: 1.8, x: statement.x, y: statement.y, z: 2.1 },
      { t: typed + 0.3, x: statement.x + 30, y: statement.y + 20, z: 2.1 },
      // Out to the Run button and the empty results.
      { t: typed + 0.95, x: 1150, y: 480, z: 1.3 },
      { t: results + 0.15, x: 1150, y: 530, z: 1.3 },
      // Down into the result.
      { t: results + 1.5, x: 760, y: 720, z: 1.75 },
      { t: select - 0.3, x: cellCenter[0] - 120, y: cellCenter[1] + 60, z: 2.2 },
      { t: commit + 0.4, x: cellCenter[0] - 120, y: cellCenter[1] + 60, z: 2.2 },
      // Over to the details cell, then onto the editor it expands into.
      { t: jsonSelect - 0.1, x: jsonPoint[0] + 60, y: jsonPoint[1] + 70, z: 2.1 },
      { t: jsonExpand, x: jsonPoint[0] + 60, y: jsonPoint[1] + 70, z: 2.1 },
      { t: jsonExpand + 0.8, x: editorCenter[0], y: editorCenter[1], z: 1.9 },
      { t: toApply, x: editorCenter[0] - 20, y: editorCenter[1] + 10, z: 1.9 },
      // Back far enough to take in the pending change and Apply edits.
      { t: toApply + 1.0, x: 820, y: 790, z: 1.5 },
      { t: outro, x: 820, y: 790, z: 1.5 },
      { t: outro + 1.3, x: WIDTH / 2, y: HEIGHT / 2, z: 1 },
      { t: duration, x: WIDTH / 2, y: HEIGHT / 2, z: 1 },
    ],
    pointer: [
      { t: 0, x: 1200, y: 860, o: 0 },
      { t: typed + 0.3, x: 1250, y: 640, o: 0 },
      { t: typed + 0.6, x: 1250, y: 640, o: 1 },
      { t: typed + 0.9, x: run.x[0] + 30, y: run.y - 4, o: 1 },
      { t: results + 0.6, x: run.x[0] + 30, y: run.y - 4, o: 1 },
      { t: select, x: cellCenter[0] + 8, y: cellCenter[1] + 2, o: 1 },
      { t: commit + 0.6, x: cellCenter[0] + 8, y: cellCenter[1] + 2, o: 1 },
      { t: jsonSelect, x: jsonPoint[0], y: jsonPoint[1], o: 1 },
      { t: jsonOpen + 0.15, x: jsonPoint[0], y: jsonPoint[1], o: 1 },
      { t: jsonExpand, x: maximizePoint[0], y: maximizePoint[1], o: 1 },
      { t: jsonExpand + 0.6, x: maximizePoint[0] + 30, y: maximizePoint[1] + 60, o: 1 },
      { t: toApply + 0.3, x: maximizePoint[0] + 30, y: maximizePoint[1] + 60, o: 1 },
      { t: toApply + 1.1, x: applyButton[0] + applyButton[2] * 0.6, y: applyButton[1] + 14, o: 1 },
      { t: outro, x: applyButton[0] + applyButton[2] * 0.6, y: applyButton[1] + 14, o: 1 },
      { t: outro + 0.5, x: applyButton[0] + applyButton[2] * 0.6, y: applyButton[1] + 14, o: 0 },
      { t: duration, x: 1200, y: 860, o: 0 },
    ],
    clicks: [
      { t: typed + 0.95, x: run.x[0] + 30, y: run.y - 4 },
      { t: select, x: cellCenter[0] + 8, y: cellCenter[1] + 2 },
      { t: open - 0.12, x: cellCenter[0] + 8, y: cellCenter[1] + 2 },
      { t: open, x: cellCenter[0] + 8, y: cellCenter[1] + 2 },
      { t: jsonSelect, x: jsonPoint[0], y: jsonPoint[1] },
      { t: jsonOpen - 0.12, x: jsonPoint[0], y: jsonPoint[1] },
      { t: jsonOpen, x: jsonPoint[0], y: jsonPoint[1] },
      { t: jsonExpand, x: maximizePoint[0], y: maximizePoint[1] },
    ],
    rings: [
      { from: typed + 0.75, to: results + 0.5, b: run.clip },
      { from: toApply + 1.1, to: outro + 0.3, b: applyButton },
    ],
    keys: [{ from: commit - 0.35, to: commit + 0.4, down: commit - 0.05, keys: ["↵"] }],
    captions: [
      { from: 0.9, to: typed + 0.5, text: "The statement under the cursor is outlined" },
      { from: typed + 0.7, to: results + 1.6, text: "Run Current executes just that statement" },
      { from: select - 0.6, to: commit + 1.0, text: "Edit results in place" },
      { from: jsonSelect - 0.2, to: toApply + 0.5, text: "JSON expands into a formatted editor" },
      { from: toApply + 0.9, to: outro + 0.6, text: "Changes wait for Apply edits" },
    ],
  };
}

/// Which keyframes show at `t`.
function layerState(plan, t) {
  const { lines } = plan;
  if (t < plan.typingStart) return { base: "01-start" };
  for (let ix = 0; ix < lines.length; ix++) {
    const line = lines[ix];
    const below = ix === 0 ? "01-start" : lines[ix - 1].key;
    if (t < line.start) return { base: below };
    if (t < line.end) {
      const shown = line.chars.filter((c) => c.t <= t).length;
      const x = shown < line.chars.length ? line.chars[shown].x : WIDTH;
      return { base: below, top: line.key, opacity: 1, reveal: { x, ...line.hidden } };
    }
  }
  if (t < plan.results) return { base: lines[lines.length - 1].key };
  if (t < plan.select) {
    return fade(lines[lines.length - 1].key, "06-results", (t - plan.results) / 0.15);
  }
  if (t < plan.open) return { base: "07-selected" };
  const value = plan.value;
  if (t < value.chars[0].t) return { base: "08-editing" };
  if (t < plan.commit) {
    const shown = value.chars.filter((c) => c.t <= t).length;
    const x = shown < value.chars.length ? value.chars[shown].x : WIDTH;
    return { base: "08-editing", top: "09-typed", opacity: 1, reveal: { x, top: value.top, bottom: value.bottom, right: value.right } };
  }
  if (t < plan.jsonSelect) return { base: "10-edited" };
  if (t < plan.jsonOpen) return { base: "11-json-selected" };
  if (t < plan.jsonExpand) return { base: "12-json-inline" };
  return fade("12-json-inline", "13-json-expanded", (t - plan.jsonExpand) / 0.2);
}

function fade(base, top, p) {
  p = clamp(p, 0, 1);
  return p >= 1 ? { base: top } : { base, top, opacity: easeInOut(p) };
}

/// Where the caret is at `t`, if it shows.
function caretState(plan, t) {
  const { lines } = plan;
  if (t < 0.9 || t >= plan.typed + 0.5) return null;
  for (const line of lines) {
    if (t < line.end) {
      const shown = line.chars.filter((c) => c.t <= t).length;
      const last = line.chars[shown - 1];
      const x = shown === 0 ? line.chars[0].x : last.x + (line.chars[1].x - line.chars[0].x);
      return {
        x,
        top: line.top + 1,
        height: line.bottom - line.top,
        blink: t < line.start,
      };
    }
  }
  const last = lines[lines.length - 1];
  const step = last.chars[1].x - last.chars[0].x;
  return {
    x: last.chars[last.chars.length - 1].x + step,
    top: last.top + 1,
    height: last.bottom - last.top,
    blink: true,
  };
}

/// The elements of the result's rows, by row, so they can come in in turn.
function rowElements(layer, snapshot, grid) {
  const rows = [];
  for (const element of layer.querySelectorAll("[data-i]")) {
    const item = snapshot.items[Number(element.dataset.i)];
    const box = item.b ?? (item.t === "text" ? [item.x[0], item.y - item.size, 0, item.size] : null);
    if (!box || box[3] > grid.rowHeight + 2) continue;
    const middle = box[1] + box[3] / 2;
    if (middle < grid.top || middle > grid.bottom || box[0] < grid.left) continue;
    const row = Math.floor((middle - grid.top) / grid.rowHeight);
    (rows[row] ??= []).push(element);
  }
  return rows.map((row) => row ?? []);
}

/// Whether path data lies in the editor, where the statement's frame is: the
/// right of the sidebar and above the results.
function inEditor(d) {
  const numbers = d.match(/-?[\d.]+/g)?.map(Number) ?? [];
  for (let ix = 0; ix + 1 < numbers.length; ix += 2) {
    const [x, y] = [numbers[ix], numbers[ix + 1]];
    if (x < 346 || y < 80 || y > 520) return false;
  }
  return numbers.length > 0;
}

function findRun(snapshot, test) {
  const run = snapshot.items.find((item) => item.t === "text" && test(item));
  if (!run) throw new Error("The keyframes do not hold the text the demo looks for");
  return run;
}

/// A clip path showing all of a layer but a hole from `x` to `right` on a
/// line, the text still to be typed.
function notch({ x, top, bottom, right }) {
  if (x >= right) return "";
  return `polygon(evenodd, 0 0, ${WIDTH}px 0, ${WIDTH}px ${HEIGHT}px, 0 ${HEIGHT}px, 0 0, ${x}px ${top}px, ${right}px ${top}px, ${right}px ${bottom}px, ${x}px ${bottom}px, ${x}px ${top}px)`;
}

/// The value of keyed `frames` at `t`, eased between keys.
function interpolate(frames, t) {
  if (t <= frames[0].t) return frames[0];
  for (let ix = 1; ix < frames.length; ix++) {
    const a = frames[ix - 1];
    const b = frames[ix];
    if (t < b.t) {
      const p = easeInOut((t - a.t) / (b.t - a.t));
      const out = {};
      for (const key of Object.keys(b)) out[key] = a[key] + (b[key] - a[key]) * p;
      return out;
    }
  }
  return frames[frames.length - 1];
}

function clamp(value, min, max) {
  return Math.min(max, Math.max(min, value));
}

function easeInOut(p) {
  return p < 0.5 ? 4 * p * p * p : 1 - (-2 * p + 2) ** 3 / 2;
}

function easeOut(p) {
  return 1 - (1 - p) ** 3;
}

let styled = false;
function injectStyles() {
  if (styled) return;
  styled = true;
  const style = document.createElement("style");
  style.textContent = `
    .bd-root { container-type: inline-size; position: relative; }
    .bd-viewport {
      position: relative; aspect-ratio: ${WIDTH} / ${HEIGHT}; overflow: hidden;
      border-radius: 12px; background: #11111b;
      box-shadow: 0 0 0 1px rgb(255 255 255 / 0.08), 0 40px 80px -30px rgb(0 0 0 / 0.6);
    }
    .bd-stage { position: absolute; left: 0; top: 0; width: ${WIDTH}px; height: ${HEIGHT}px; transform-origin: 0 0; }
    .bd-camera { position: absolute; inset: 0; transform-origin: 0 0; will-change: transform; }
    .bd-layer { position: absolute; inset: 0; }
    .bd-layer svg { display: block; }
    .bd-layer [data-i] { transform-box: fill-box; }
    .bd-overlay { position: absolute; inset: 0; pointer-events: none; }
    .bd-overlay > * { position: absolute; left: 0; top: 0; }
    .bd-caret { background: ${CARET}; border-radius: 1px; }
    .bd-pointer { width: 18px; height: 25px; filter: drop-shadow(0 2px 3px rgb(0 0 0 / 0.4)); }
    .bd-ripple { border-radius: 50%; background: #cba6f7; }
    .bd-ring { border: 2px solid #cba6f7; border-radius: 8px; box-shadow: 0 0 0 4px rgb(203 166 247 / 0.18); }
    .bd-keys {
      left: 50%; top: auto; bottom: 18%; display: flex; gap: 8px;
    }
    .bd-keys kbd {
      font: 600 clamp(14px, 2.4cqw, 26px)/1 system-ui, -apple-system, sans-serif;
      min-width: 1.9em; padding: 0.45em 0.6em; text-align: center; color: #cdd6f4;
      background: #313244; border-radius: 0.35em;
      box-shadow: 0 0.18em 0 #181825, 0 10px 24px -8px rgb(0 0 0 / 0.6), inset 0 1px 0 rgb(255 255 255 / 0.08);
      transition: transform 80ms, box-shadow 80ms;
    }
    .bd-keys.bd-pressed kbd { transform: translateY(0.12em); box-shadow: 0 0.06em 0 #181825, 0 6px 16px -8px rgb(0 0 0 / 0.6); }
    .bd-caption {
      left: 50%; top: 4.5%; transform: translateX(-50%);
      font: 500 clamp(12px, 1.7cqw, 19px)/1.2 system-ui, -apple-system, sans-serif;
      color: #cdd6f4; white-space: nowrap; padding: 0.6em 1em; border-radius: 999px;
      background: rgb(17 17 27 / 0.82); backdrop-filter: blur(8px);
      box-shadow: 0 0 0 1px rgb(255 255 255 / 0.08), 0 12px 28px -12px rgb(0 0 0 / 0.7);
    }
    .bd-caption:empty { display: none; }
    /* The hint shows for a while, then clears the tabs it covers. */
    .bd-ended .bd-caption { animation: bd-hint 5s 0.9s both; }

    .bd-dim { inset: 0; width: 100%; height: 100%; opacity: 0; transition: opacity 0.25s; }
    .bd-dim.bd-on { opacity: 1; }

    .bd-replay {
      left: auto; top: auto; right: 2.2%; bottom: 3.5%; display: none; align-items: center; gap: 0.45em;
      font: 500 clamp(11px, 1.3cqw, 15px)/1 system-ui, -apple-system, sans-serif; color: #cdd6f4;
      padding: 0.6em 0.9em; border: 0; border-radius: 999px; cursor: pointer; pointer-events: auto;
      background: rgb(49 50 68 / 0.9); box-shadow: 0 0 0 1px rgb(255 255 255 / 0.08), 0 10px 24px -10px rgb(0 0 0 / 0.7);
    }
    .bd-replay svg { width: 1.1em; height: 1.1em; }
    .bd-replay:hover { background: #45475a; }
    .bd-ended .bd-replay { display: inline-flex; animation: bd-in 0.4s 1.2s both; }

    .bd-spots { position: absolute; inset: 0; pointer-events: none; }
    .bd-spot { position: absolute; display: none; z-index: 1; }
    .bd-spot.bd-open { z-index: 2; }
    .bd-ended .bd-spot { display: block; animation: bd-pop 0.45s calc(0.25s + var(--i) * 0.09s) both; }
    .bd-dot {
      position: absolute; width: 16px; height: 16px; margin: -8px 0 0 -8px; padding: 0;
      border: 0; border-radius: 50%; cursor: pointer; pointer-events: auto;
      background: radial-gradient(circle, #fff 0 22%, #cba6f7 32% 100%);
      box-shadow: 0 0 0 0 rgb(203 166 247 / 0.7), 0 0 14px 2px rgb(203 166 247 / 0.65);
      animation: bd-glow 2s calc(var(--i) * -0.3s) infinite;
    }
    .bd-dot:focus-visible { outline: 2px solid #f5e0dc; outline-offset: 3px; }
    .bd-open .bd-dot { animation: none; box-shadow: 0 0 0 5px rgb(203 166 247 / 0.3), 0 0 18px 4px rgb(203 166 247 / 0.8); }
    .bd-card {
      position: absolute; width: min(270px, 70vw); box-sizing: border-box; padding: 14px 16px;
      pointer-events: auto; visibility: hidden; opacity: 0; transition: opacity 0.18s, transform 0.18s, visibility 0.18s;
      font: 400 13.5px/1.45 system-ui, -apple-system, sans-serif; color: #bac2de;
      background: rgb(30 30 46 / 0.97); border-radius: 12px;
      box-shadow: 0 0 0 1px rgb(203 166 247 / 0.35), 0 24px 48px -16px rgb(0 0 0 / 0.75);
    }
    .bd-card.bd-wide { width: min(360px, 80vw); }
    .bd-media {
      margin: -4px -6px 12px; border-radius: 8px; overflow: hidden; background: #11111b;
      box-shadow: 0 0 0 1px rgb(255 255 255 / 0.08);
    }
    .bd-media svg { display: block; width: 100%; height: 100%; }
    .bd-card strong { display: block; font-weight: 600; font-size: 14.5px; color: #cdd6f4; margin-bottom: 4px; }
    .bd-card p { margin: 0 0 8px; }
    .bd-card a { color: #cba6f7; font-weight: 500; text-decoration: none; }
    .bd-card a:hover { text-decoration: underline; }
    .bd-card a::after { content: " →"; }
    .bd-spot[data-side="right"] .bd-card { left: 16px; }
    .bd-spot[data-side="left"] .bd-card { right: 16px; }
    .bd-spot[data-rise="down"] .bd-card { top: -18px; }
    .bd-spot[data-rise="up"] .bd-card { bottom: -18px; }
    .bd-spot[data-side="right"] .bd-card { transform: translateX(-6px); }
    .bd-spot[data-side="left"] .bd-card { transform: translateX(6px); }
    .bd-open .bd-card { visibility: visible; opacity: 1; transform: none !important; }

    @keyframes bd-glow {
      0% { box-shadow: 0 0 0 0 rgb(203 166 247 / 0.65), 0 0 14px 2px rgb(203 166 247 / 0.6); }
      70% { box-shadow: 0 0 0 12px rgb(203 166 247 / 0), 0 0 14px 2px rgb(203 166 247 / 0.6); }
      100% { box-shadow: 0 0 0 0 rgb(203 166 247 / 0), 0 0 14px 2px rgb(203 166 247 / 0.6); }
    }
    @keyframes bd-pop { from { opacity: 0; transform: scale(0.4); } to { opacity: 1; transform: none; } }
    @keyframes bd-in { from { opacity: 0; } }
    @keyframes bd-hint { 0% { opacity: 0; } 10%, 80% { opacity: 0.9; } 100% { opacity: 0; } }
    @media (prefers-reduced-motion: reduce) {
      .bd-dot, .bd-ended .bd-spot, .bd-ended .bd-caption, .bd-ended .bd-replay { animation: none; }
    }
  `;
  document.head.append(style);
}
