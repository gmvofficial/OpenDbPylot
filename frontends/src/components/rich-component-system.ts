// The rich-component system — architecture:
//   ComponentRegistry  maps a component `type` -> a renderer
//   ComponentManager   applies a lifecycle (create/update/replace/remove) by `id`
// This is what lets the backend stream live UI (a table appears, a chart renders,
// a status updates), each addressed by a stable component id.

import "./plotly-chart";
import { richStyles } from "../styles/rich-component-styles";
import type { RichComponent } from "../services/api-client";

function escapeHtml(s: string): string {
  return s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]!));
}

export interface ComponentRenderer {
  render(c: RichComponent): HTMLElement;
}

class TextRenderer implements ComponentRenderer {
  render(c: RichComponent): HTMLElement {
    const el = document.createElement("div");
    el.className = "rc rc-text";
    el.dataset.id = c.id;
    const text = String(c.data.text ?? "");
    if (c.data.language) {
      el.innerHTML =
        `<div class="rc-label">${escapeHtml(c.data.title || "Code")}</div>` +
        `<div class="rc-codewrap"><button class="rc-copy">copy</button>` +
        `<pre class="rc-code">${escapeHtml(text)}</pre></div>`;
      const btn = el.querySelector(".rc-copy") as HTMLButtonElement;
      btn.onclick = () => {
        navigator.clipboard.writeText(text);
        btn.textContent = "copied";
        setTimeout(() => (btn.textContent = "copy"), 1200);
      };
    } else {
      el.innerHTML = `<div class="rc-prose">${escapeHtml(text)}</div>`;
    }
    return el;
  }
}

class DataFrameRenderer implements ComponentRenderer {
  render(c: RichComponent): HTMLElement {
    const cols: any[] = c.data.columns || [];
    const rows: any[][] = c.data.rows || [];
    const el = document.createElement("div");
    el.className = "rc rc-df";
    el.dataset.id = c.id;
    let h = `<div class="rc-label">${escapeHtml(c.data.title || "Result")}</div><div class="rc-tablewrap"><table><thead><tr>`;
    for (const col of cols) h += `<th>${escapeHtml(String(col))}</th>`;
    h += "</tr></thead><tbody>";
    for (const r of rows) {
      h += "<tr>";
      for (const cell of r) h += `<td>${escapeHtml(String(cell))}</td>`;
      h += "</tr>";
    }
    h += `</tbody></table></div><div class="rc-rowcount">${rows.length} row(s)</div>`;
    el.innerHTML = h;
    return el;
  }
}

class ChartRenderer implements ComponentRenderer {
  render(c: RichComponent): HTMLElement {
    const el = document.createElement("div");
    el.className = "rc rc-chart";
    el.dataset.id = c.id;
    const chart = document.createElement("plotly-chart") as any;
    chart.spec = c.data.spec;
    el.appendChild(chart);
    return el;
  }
}

class NotificationRenderer implements ComponentRenderer {
  render(c: RichComponent): HTMLElement {
    const el = document.createElement("div");
    el.className = `rc rc-note rc-note-${c.data.level || "info"}`;
    el.dataset.id = c.id;
    el.textContent = String(c.data.message ?? "");
    return el;
  }
}

class StatusIndicatorRenderer implements ComponentRenderer {
  render(c: RichComponent): HTMLElement {
    const el = document.createElement("div");
    el.className = "rc rc-status";
    el.dataset.id = c.id;
    el.textContent = String(c.data.message ?? "");
    return el;
  }
}

class ProgressBarRenderer implements ComponentRenderer {
  render(c: RichComponent): HTMLElement {
    const el = document.createElement("div");
    el.className = "rc rc-progress";
    el.dataset.id = c.id;
    const label = String(c.data.label ?? "");
    const value = Math.max(0, Math.min(100, Number(c.data.value) || 0));
    el.innerHTML =
      `<div class="rc-progress-label"><span>${escapeHtml(label)}</span><span>${value}%</span></div>` +
      `<div class="rc-progress-track"><div class="rc-progress-fill" style="width:${value}%"></div></div>`;
    return el;
  }
}

class CardRenderer implements ComponentRenderer {
  render(c: RichComponent): HTMLElement {
    const el = document.createElement("div");
    el.className = "rc rc-card";
    el.dataset.id = c.id;
    let h = "";
    if (c.data.title) h += `<div class="rc-card-title">${escapeHtml(String(c.data.title))}</div>`;
    if (c.data.body) h += `<div class="rc-card-body">${escapeHtml(String(c.data.body))}</div>`;
    if (Array.isArray(c.data.items)) {
      for (const item of c.data.items) h += `<div class="rc-card-item">• ${escapeHtml(String(item))}</div>`;
    }
    el.innerHTML = h;
    return el;
  }
}

class ButtonsRenderer implements ComponentRenderer {
  render(c: RichComponent): HTMLElement {
    const el = document.createElement("div");
    el.className = "rc rc-buttons";
    el.dataset.id = c.id;
    const buttons: any[] = Array.isArray(c.data.buttons) ? c.data.buttons : [c.data];
    for (const b of buttons) {
      const btn = document.createElement("button");
      btn.className = "rc-btn";
      btn.textContent = b.label || "Action";
      btn.onclick = () => {
        // Bubbles up to <opendbpylot-chat>, which runs the prompt.
        el.dispatchEvent(
          new CustomEvent("opendbpylot-action", {
            bubbles: true,
            composed: true,
            detail: { action: b.action, prompt: b.prompt, label: b.label },
          })
        );
      };
      el.appendChild(btn);
    }
    return el;
  }
}

export class ComponentRegistry {
  private renderers = new Map<string, ComponentRenderer>();

  constructor() {
    this.register("text", new TextRenderer());
    this.register("dataframe", new DataFrameRenderer());
    this.register("chart", new ChartRenderer());
    this.register("notification", new NotificationRenderer());
    this.register("status_indicator", new StatusIndicatorRenderer());
    this.register("progress_bar", new ProgressBarRenderer());
    this.register("card", new CardRenderer());
    const buttons = new ButtonsRenderer();
    this.register("button", buttons);
    this.register("button_group", buttons);
  }

  register(type: string, renderer: ComponentRenderer) {
    this.renderers.set(type, renderer);
  }

  render(c: RichComponent): HTMLElement {
    const renderer = this.renderers.get(c.type);
    if (!renderer) {
      const f = document.createElement("div");
      f.className = "rc rc-fallback";
      f.textContent = `[unknown component: ${c.type}]`;
      return f;
    }
    return renderer.render(c);
  }
}

// Component types that update chrome (status bar, input) rather than the message list.
const UI_STATE_TYPES = new Set(["status_bar_update", "chat_input_update", "task_tracker_update"]);

export class ComponentManager {
  private registry = new ComponentRegistry();
  private elements = new Map<string, HTMLElement>();

  constructor(
    private container: HTMLElement,
    private onUiState: (c: RichComponent) => void
  ) {
    this.ensureStyles();
  }

  private ensureStyles() {
    if (this.container.querySelector("style[data-rc]")) return;
    const style = document.createElement("style");
    style.setAttribute("data-rc", "1");
    style.textContent = richStyles;
    this.container.prepend(style);
  }

  /** Apply one streamed component to the DOM, respecting its lifecycle. */
  processChunk(rich: RichComponent) {
    if (UI_STATE_TYPES.has(rich.type)) {
      this.onUiState(rich);
      return;
    }

    const op = rich.lifecycle || "create";
    const existing = this.elements.get(rich.id);

    if (op === "remove") {
      existing?.remove();
      this.elements.delete(rich.id);
      return;
    }

    const el = this.registry.render(rich);
    if (existing && (op === "update" || op === "replace")) {
      existing.replaceWith(el);
    } else {
      this.container.appendChild(el);
    }
    this.elements.set(rich.id, el);
  }
}
