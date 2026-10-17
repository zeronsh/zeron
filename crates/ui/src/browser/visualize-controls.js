// Minimal compatibility for self-contained visualize fragments. This API only
// changes this preview's memory; it has no connection to a model or the host.
(() => {
  "use strict";
  let widgetState = {};
  const copy = value => JSON.parse(JSON.stringify(value));
  const refresh = () => window.dispatchEvent(new CustomEvent("openai:set_globals", {
    detail: { globals: { widgetState: copy(widgetState) } }
  }));
  Object.defineProperty(window, "openai", { value: Object.freeze({
    get widgetState() { return copy(widgetState); },
    async setWidgetState(value) {
      if (!value || typeof value !== "object" || Array.isArray(value)) throw TypeError("Expected a state object");
      const serialized = JSON.stringify(value);
      if (new TextEncoder().encode(serialized).byteLength > 65536) throw RangeError("Preview state is too large");
      const next = JSON.parse(serialized);
      if (!next || typeof next !== "object" || Array.isArray(next)) throw TypeError("Expected a state object");
      widgetState = next;
      await new Promise(resolve => queueMicrotask(() => { refresh(); resolve(); }));
    }
  }) });

  class Tweak {
    constructor({ container = document.body, onChange = () => {} } = {}) {
      this.onChange = onChange;
      this.controls = [];
      this.host = document.createElement("div");
      this.host.className = "zeron-visualize-controls";
      const shadow = this.host.attachShadow({ mode: "open" });
      const style = document.createElement("style");
      style.textContent = `:host{display:block;margin-top:16px;color-scheme:light dark;font:13px system-ui}details{border:1px solid #8886;border-radius:8px;padding:10px 12px}summary{cursor:pointer;font-weight:600}label{display:flex;align-items:center;justify-content:space-between;gap:16px;margin-top:12px}select{max-width:65%;font:inherit;padding:4px}input{accent-color:#198878}`;
      this.panel = document.createElement("details");
      this.panel.open = true;
      const summary = document.createElement("summary");
      summary.textContent = "Design controls";
      this.panel.append(summary);
      shadow.append(style, this.panel);
      container.append(this.host);
      // Application listeners may update their bound objects before or after
      // this listener. Refresh inputs after every listener has seen globals.
      this.sync = () => queueMicrotask(() => this.refresh());
      window.addEventListener("openai:set_globals", this.sync);
    }
    row(label, control) {
      const row = document.createElement("label");
      const text = document.createElement("span");
      text.textContent = label;
      row.append(text, control);
      this.panel.append(row);
    }
    addSelect(object, key, { label = key, options = [] } = {}) {
      const entries = Array.isArray(options)
        ? options.map(value => [String(value), value]) : Object.entries(options);
      const select = document.createElement("select");
      entries.forEach(([label], index) => {
        const option = document.createElement("option");
        option.textContent = label;
        option.value = String(index);
        select.append(option);
      });
      const sync = () => { select.selectedIndex = entries.findIndex(([, value]) => Object.is(value, object[key])); };
      select.addEventListener("change", () => {
        if (select.selectedIndex < 0) return;
        object[key] = entries[select.selectedIndex][1];
        this.onChange(object, key);
        this.refresh();
      });
      this.controls.push(sync);
      this.row(label, select);
      sync();
      return select;
    }
    addToggle(object, key, { label = key } = {}) {
      const toggle = document.createElement("input");
      toggle.type = "checkbox";
      const sync = () => { toggle.checked = Boolean(object[key]); };
      toggle.addEventListener("change", () => {
        object[key] = toggle.checked;
        this.onChange(object, key);
        this.refresh();
      });
      this.controls.push(sync);
      this.row(label, toggle);
      sync();
      return toggle;
    }
    refresh() { this.controls.forEach(sync => sync()); }
    dispose() {
      window.removeEventListener("openai:set_globals", this.sync);
      this.host.remove();
    }
  }
  Object.defineProperty(globalThis, "Tweak", { value: Tweak });
})();
