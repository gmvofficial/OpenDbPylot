import { LitElement, html, css } from "lit";
import { customElement, property } from "lit/decorators.js";
import Plotly from "plotly.js-dist-min";

// A small Lit wrapper around Plotly. Set the `spec` property ({ data, layout }).
// The chart is responsive and shows a toolbar with: download PNG, zoom, pan,
// autoscale, and reset — so the user can resize/export the chart.
@customElement("plotly-chart")
export class PlotlyChart extends LitElement {
  static styles = css`
    :host { display: block; }
    .chart { width: 100%; height: 360px; }
    /* Make Plotly's toolbar legible on the dark surface. */
    .chart .modebar { background: transparent !important; }
  `;

  @property({ attribute: false }) spec: any = null;

  private chartEl?: HTMLDivElement;
  private resizeObserver?: ResizeObserver;

  render() {
    return html`<div class="chart"></div>`;
  }

  firstUpdated() {
    this.chartEl = this.renderRoot.querySelector(".chart") as HTMLDivElement;
    this.draw();
    // Keep the chart width in sync with its container.
    this.resizeObserver = new ResizeObserver(() => {
      if (this.chartEl && this.spec) {
        Plotly.relayout(this.chartEl, { width: this.chartEl.offsetWidth });
      }
    });
    this.resizeObserver.observe(this.chartEl);
  }

  disconnectedCallback() {
    super.disconnectedCallback();
    this.resizeObserver?.disconnect();
  }

  updated() {
    this.draw();
  }

  private cssVar(name: string, fallback: string): string {
    const v = getComputedStyle(this).getPropertyValue(name).trim();
    return v || fallback;
  }

  private draw() {
    if (!this.chartEl || !this.spec) return;

    const muted = this.cssVar("--opendbpylot-foreground-dimmest", "#94a3b8");
    const grid = this.cssVar("--opendbpylot-outline-dimmer", "rgba(148,163,184,0.15)");

    const data = this.spec.data || [];
    const layout = Object.assign(
      {
        margin: { t: 32, r: 16, b: 48, l: 56 },
        paper_bgcolor: "rgba(0,0,0,0)",
        plot_bgcolor: "rgba(0,0,0,0)",
        font: { color: muted, size: 12 },
        xaxis: { gridcolor: grid, zerolinecolor: grid },
        yaxis: { gridcolor: grid, zerolinecolor: grid },
        colorway: ["#15a8a8", "#fe5d26", "#bf1363", "#023d60"],
        modebar: { bgcolor: "rgba(0,0,0,0)", color: muted, activecolor: "#15a8a8", orientation: "h" },
      },
      this.spec.layout || {}
    );

    const config = {
      responsive: true,
      displaylogo: false,
      // Show the toolbar on hover; keep only the useful buttons.
      displayModeBar: true,
      modeBarButtonsToRemove: ["lasso2d", "select2d"],
      // High-resolution PNG export.
      toImageButtonOptions: { format: "png", filename: "opendbpylot-chart", scale: 2 },
    };

    Plotly.newPlot(this.chartEl, data, layout, config);
  }
}
