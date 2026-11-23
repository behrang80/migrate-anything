// Code rain: columns of falling digits. In the background it is ambient decoration; on the
// migration screen it is driven by real progress — intensity follows records per second and
// the brighter vertical "words" are the field names currently being migrated.
//
// Accessibility: honours prefers-reduced-motion and a user toggle (a single still frame is drawn
// instead), pauses when the tab is hidden, and is always aria-hidden: every number it stands for
// is also shown as text next to it.

const DIGITS = "0123456789";
const EXTRA = "01<>=+*:.|/";

const css = (name, fallback) => getComputedStyle(document.documentElement).getPropertyValue(name).trim() || fallback;

export class CodeRain {
  constructor(canvas, { fontSize = 16, fade = 0.085, fps = 30 } = {}) {
    this.canvas = canvas;
    this.ctx = canvas.getContext("2d", { alpha: false });
    this.fontSize = fontSize;
    this.fade = fade;
    this.frameMs = 1000 / fps;
    this.intensity = 0.3; // 0..1: how many columns are raining and how fast
    this.target = 0.3;
    this.words = [];
    this.cols = [];
    this.running = false;
    this.last = 0;
    this.enabled = true;
    this.reduced = window.matchMedia("(prefers-reduced-motion: reduce)");
    this.reduced.addEventListener?.("change", () => this.refresh());
    this.onVisibility = () => (document.hidden ? this.stop() : this.refresh());
    document.addEventListener("visibilitychange", this.onVisibility);
    this.observer = new ResizeObserver(() => this.resize());
    this.observer.observe(canvas);
    this.resize();
  }

  /** 0 = a quiet drizzle, 1 = a full downpour. Changes ease in over a few frames. */
  setIntensity(v) {
    this.target = Math.max(0, Math.min(1, v));
  }

  /** Field names (or any short strings) to spell out in brighter columns. */
  setWords(words) {
    this.words = (words || []).filter((w) => w && w.length <= 40).slice(0, 200);
  }

  setEnabled(on) {
    this.enabled = on;
    this.refresh();
  }

  get animated() {
    return this.enabled && !this.reduced.matches && !document.hidden;
  }

  refresh() {
    if (this.animated) this.start();
    else {
      this.stop();
      this.still();
    }
  }

  resize() {
    const dpr = Math.min(window.devicePixelRatio || 1, 2);
    const w = Math.max(1, Math.floor(this.canvas.clientWidth * dpr));
    const h = Math.max(1, Math.floor(this.canvas.clientHeight * dpr));
    if (w === this.canvas.width && h === this.canvas.height) return;
    this.canvas.width = w;
    this.canvas.height = h;
    this.cell = Math.round(this.fontSize * dpr);
    this.rows = Math.ceil(h / this.cell) + 1;
    const n = Math.ceil(w / this.cell);
    this.cols = Array.from({ length: n }, () => this.newColumn(true));
    this.bg = css("--bg", "#020604");
    this.green = css("--green", "#00ff66");
    this.head = css("--focus", "#eafff2");
    this.wordColor = css("--green-soft", "#5dffa0");
    this.ctx.fillStyle = this.bg;
    this.ctx.fillRect(0, 0, w, h);
    this.ctx.font = `${this.cell}px ${css("--font", "monospace")}`;
    this.ctx.textBaseline = "top";
    // a resize clears the canvas: fast-forward so the rain does not restart from an empty sky
    if (this.animated) {
      const keep = this.intensity;
      this.intensity = Math.max(keep, this.target);
      for (let i = 0; i < 160; i++) this.step();
    }
    this.refresh();
  }

  newColumn(scatter) {
    return { y: scatter ? -Math.random() * this.rows : 0, speed: 0.35 + Math.random() * 0.75, active: false, word: null, wi: 0, prev: "" };
  }

  spawn(col) {
    col.active = true;
    col.y = -Math.floor(Math.random() * 6);
    col.speed = 0.35 + Math.random() * 0.75;
    col.prev = "";
    // roughly one column in five carries a real field name while data is flowing
    col.word = this.words.length && Math.random() < 0.22 ? this.words[Math.floor(Math.random() * this.words.length)] : null;
    col.wi = 0;
  }

  glyph(col) {
    if (col.word) {
      const ch = col.word[col.wi % (col.word.length + 3)];
      col.wi += 1;
      return ch || DIGITS[Math.floor(Math.random() * 10)];
    }
    const set = Math.random() < 0.86 ? DIGITS : EXTRA;
    return set[Math.floor(Math.random() * set.length)];
  }

  step() {
    const { ctx, cell } = this;
    this.intensity += (this.target - this.intensity) * 0.08;
    ctx.fillStyle = this.bg;
    ctx.globalAlpha = this.fade;
    ctx.fillRect(0, 0, this.canvas.width, this.canvas.height);
    ctx.globalAlpha = 1;
    const spawnChance = 0.004 + this.intensity * 0.09;
    const speedBoost = 0.55 + this.intensity * 1.1;
    for (let i = 0; i < this.cols.length; i++) {
      const col = this.cols[i];
      if (!col.active) {
        if (Math.random() < spawnChance) this.spawn(col);
        continue;
      }
      const before = Math.floor(col.y);
      col.y += col.speed * speedBoost;
      const now = Math.floor(col.y);
      if (now === before) continue;
      const x = i * cell;
      // repaint the previous head in body green, then draw the new bright head
      if (col.prev && before >= 0) {
        ctx.fillStyle = col.word ? this.wordColor : this.green;
        ctx.fillText(col.prev, x, before * cell);
      }
      const ch = this.glyph(col);
      if (now >= 0) {
        ctx.fillStyle = this.head;
        ctx.fillText(ch, x, now * cell);
      }
      col.prev = ch;
      if (now > this.rows + 2) col.active = false;
    }
  }

  /** One calm frame for reduced motion / animation switched off. */
  still() {
    const { ctx, cell } = this;
    if (!cell) return;
    ctx.fillStyle = this.bg;
    ctx.fillRect(0, 0, this.canvas.width, this.canvas.height);
    ctx.fillStyle = this.green;
    for (let i = 0; i < this.cols.length; i += 2) {
      const len = 3 + ((i * 7) % 9);
      const top = (i * 13) % Math.max(1, this.rows - len);
      for (let r = 0; r < len; r++) {
        ctx.globalAlpha = 0.08 + (r / len) * 0.3;
        ctx.fillText(DIGITS[(i + r * 3) % 10], i * cell, (top + r) * cell);
      }
    }
    ctx.globalAlpha = 1;
  }

  start() {
    if (this.running) return;
    this.running = true;
    const loop = (t) => {
      if (!this.running) return;
      if (t - this.last >= this.frameMs) {
        this.last = t;
        this.step();
      }
      this.raf = requestAnimationFrame(loop);
    };
    this.raf = requestAnimationFrame(loop);
  }

  stop() {
    this.running = false;
    if (this.raf) cancelAnimationFrame(this.raf);
  }

  destroy() {
    this.stop();
    this.observer.disconnect();
    document.removeEventListener("visibilitychange", this.onVisibility);
  }
}
