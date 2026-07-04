import { LitElement, html, css, nothing } from "lit";
import { customElement, property, state, query } from "lit/decorators.js";

import { tokens } from "../styles/design-tokens";
import { OpenDbPylotApiClient } from "../services/api-client";
import type { ChatStreamChunk, RichComponent, Transport } from "../services/api-client";
import { ComponentManager } from "./rich-component-system";

const DEFAULT_EXAMPLES = [
  "How many users are there per country?",
  "What are the names of users from the USA?",
  "How many users in total?",
  "List users created after 2024-06-01",
];

type StepStatus = "pending" | "running" | "done" | "error";

interface Step {
  id: string;
  label: string;
  detail?: string;
  status: StepStatus;
}

@customElement("opendbpylot-chat")
export class OpenDbPylotChat extends LitElement {
  static styles = [tokens, css`
    :host {
      display: flex; flex-direction: column;
      height: 100%; background: var(--opendbpylot-background-root);
      color: var(--opendbpylot-foreground-default);
      font-family: var(--opendbpylot-font-family-default);
      overflow: hidden;
    }

    .chat-container { display: flex; flex-direction: column; height: 100%; overflow: hidden; }

    /* ── Header ── */
    .header {
      display: flex; align-items: center; gap: 12px;
      padding: 14px 18px;
      border-bottom: 1px solid var(--opendbpylot-outline-dimmer);
      background: var(--opendbpylot-background-higher); flex-shrink: 0;
    }
    .logo {
      width: 26px; height: 26px; border-radius: 7px;
      background: linear-gradient(135deg, var(--opendbpylot-teal), var(--opendbpylot-navy));
    }
    .titles h1 { margin: 0; font-size: 16px; font-weight: 700; }
    .titles p { margin: 1px 0 0; font-size: 12.5px; color: var(--opendbpylot-foreground-dimmest); }
    .status {
      margin-left: auto; display: flex; align-items: center; gap: 8px;
      font-size: 12px; color: var(--opendbpylot-foreground-dimmest);
      border: 1px solid var(--opendbpylot-outline-dimmer); border-radius: 999px;
      padding: 5px 11px; background: var(--opendbpylot-background-root);
    }
    .dot { width: 7px; height: 7px; border-radius: 50%; background: var(--opendbpylot-foreground-dimmest); flex-shrink: 0; }
    .dot.working { background: var(--opendbpylot-teal); animation: pulse 1s infinite; }
    .dot.idle, .dot.success { background: var(--opendbpylot-teal); }
    .dot.error { background: var(--opendbpylot-accent-negative-default); }
    @keyframes pulse { 0%,100% { opacity: 1; } 50% { opacity: .35; } }

    .btn-toggle-panel {
      border: 1px solid var(--opendbpylot-outline-dimmer);
      background: var(--opendbpylot-background-root); color: var(--opendbpylot-foreground-dimmest);
      border-radius: 6px; padding: 4px 9px; font-size: 12px;
      cursor: pointer; font-family: var(--opendbpylot-font-family-default); margin-left: 8px;
    }
    .btn-toggle-panel:hover { color: var(--opendbpylot-foreground-default); }

    /* ── Body ── */
    .chat-body { display: flex; flex: 1; min-height: 0; overflow: hidden; }
    .chat-main { flex: 1; display: flex; flex-direction: column; min-width: 0; overflow: hidden; }

    .messages {
      flex: 1; overflow-y: auto; padding: 20px;
      display: flex; flex-direction: column; gap: 16px; min-height: 0;
      overflow-x: hidden;
    }
    .messages::-webkit-scrollbar { width: 4px; }
    .messages::-webkit-scrollbar-thumb { background: var(--opendbpylot-outline-dimmer); border-radius: 4px; }
    .messages::-webkit-scrollbar-track { background: transparent; }

    .welcome {
      border: 1px solid var(--opendbpylot-outline-dimmer); border-radius: 14px;
      background: var(--opendbpylot-background-higher); padding: 20px;
    }
    .welcome h2 { margin: 0 0 4px; font-size: 15px; font-weight: 700; }
    .welcome p { margin: 0 0 14px; font-size: 13px; color: var(--opendbpylot-foreground-dimmest); }
    .chips { display: flex; flex-wrap: wrap; gap: 8px; }
    .chip {
      cursor: pointer; border: 1px solid var(--opendbpylot-outline-dimmer); border-radius: 10px;
      background: var(--opendbpylot-background-highest); color: var(--opendbpylot-foreground-default);
      padding: 8px 12px; font-size: 13px; transition: border-color 0.12s;
    }
    .chip:hover { border-color: var(--opendbpylot-teal); }

    .turn { width: 100%; }
    .msg-user {
      align-self: flex-end; max-width: 85%;
      background: rgba(21,168,168,.1); border: 1px solid rgba(21,168,168,.2);
      padding: 10px 14px; border-radius: 14px 14px 4px 14px;
      font-size: 14px; margin-left: auto; width: fit-content;
    }
    .bot-card {
      border: 1px solid var(--opendbpylot-outline-dimmer); border-radius: 14px 14px 14px 4px;
      background: var(--opendbpylot-background-higher); padding: 14px; box-shadow: var(--opendbpylot-shadow-lg);
    }

    .history-sql {
      background: var(--opendbpylot-background-highest);
      border: 1px solid var(--opendbpylot-outline-dimmer); border-radius: 8px;
      padding: 12px 14px; font-family: monospace; font-size: 13px;
      white-space: pre-wrap; word-break: break-all;
      color: var(--opendbpylot-foreground-default); margin: 0;
    }
    .history-rerun {
      margin-top: 10px; padding: 7px 14px; border-radius: 8px;
      border: 1px solid var(--opendbpylot-outline-dimmer);
      background: var(--opendbpylot-background-highest);
      color: var(--opendbpylot-foreground-default);
      font-size: 12.5px; font-family: var(--opendbpylot-font-family-default); cursor: pointer;
    }
    .history-rerun:hover { border-color: var(--opendbpylot-teal); }

    .composer {
      flex-shrink: 0; padding: 14px 18px 18px;
      background: var(--opendbpylot-background-root);
      border-top: 1px solid var(--opendbpylot-outline-dimmer);
    }
    .composer form {
      display: flex; gap: 10px; align-items: center;
      border: 1px solid var(--opendbpylot-outline-dimmer); border-radius: 14px;
      background: var(--opendbpylot-background-higher); padding: 8px 8px 8px 14px;
      transition: border-color 0.15s;
    }
    .composer form:focus-within { border-color: var(--opendbpylot-teal); }
    input {
      flex: 1; border: none; outline: none; background: transparent;
      color: var(--opendbpylot-foreground-default); font-size: 14.5px;
      font-family: var(--opendbpylot-font-family-default);
    }
    button.send {
      border: none; border-radius: 10px; cursor: pointer; color: #fff;
      background: var(--opendbpylot-teal);
      font-weight: 600; font-size: 14px; padding: 9px 18px;
      font-family: var(--opendbpylot-font-family-default);
    }
    button.send:disabled { opacity: .5; cursor: default; }

    /* ── Activity Panel ── */
    .chat-activity-panel {
      width: 260px; flex-shrink: 0;
      border-left: 1px solid var(--opendbpylot-outline-dimmer);
      background: var(--opendbpylot-background-higher);
      display: flex; flex-direction: column;
      transition: width 0.2s ease, opacity 0.2s;
      overflow: hidden;
    }
    .chat-activity-panel.hidden { width: 0; opacity: 0; border-left: none; }

    .activity-header {
      display: flex; align-items: center; justify-content: space-between;
      padding: 14px 16px 12px;
      border-bottom: 1px solid var(--opendbpylot-outline-dimmer);
      flex-shrink: 0;
    }
    .activity-title {
      font-size: 11px; font-weight: 700; text-transform: uppercase;
      letter-spacing: 0.09em; color: var(--opendbpylot-foreground-dimmest);
    }
    .activity-close {
      border: none; background: transparent;
      color: var(--opendbpylot-foreground-dimmest); cursor: pointer;
      font-size: 16px; line-height: 1; padding: 0 2px;
      font-family: var(--opendbpylot-font-family-default);
    }
    .activity-close:hover { color: var(--opendbpylot-foreground-default); }

    .activity-body { padding: 20px 16px; flex: 1; overflow-y: auto; }

    /* ── Pipeline Stepper ── */
    .pipeline {
      display: flex; flex-direction: column;
    }

    .pipeline-step {
      display: flex; flex-direction: column; align-items: flex-start; position: relative;
    }

    .pipeline-step-row {
      display: flex; align-items: center; gap: 12px; width: 100%;
    }

    /* The circle node */
    .step-circle {
      width: 32px; height: 32px; border-radius: 50%;
      flex-shrink: 0; position: relative; z-index: 1;
      display: flex; align-items: center; justify-content: center;
      transition: background 0.3s, border-color 0.3s;
    }
    .step-circle.pending {
      border: 2px solid var(--opendbpylot-outline-dimmer);
      background: var(--opendbpylot-background-highest);
    }
    .step-circle.running {
      border: 2px solid var(--opendbpylot-teal);
      background: rgba(21,168,168,.1);
    }
    .step-circle.done {
      border: 2px solid var(--opendbpylot-teal);
      background: var(--opendbpylot-teal);
    }
    .step-circle.error {
      border: 2px solid var(--opendbpylot-accent-negative-default);
      background: rgba(248,113,113,.1);
    }

    /* Spinner ring for running state */
    .step-circle.running::before {
      content: '';
      position: absolute;
      inset: -4px;
      border-radius: 50%;
      border: 2px solid transparent;
      border-top-color: var(--opendbpylot-teal);
      animation: step-spin 0.8s linear infinite;
    }
    @keyframes step-spin { to { transform: rotate(360deg); } }

    .step-circle svg {
      width: 14px; height: 14px;
    }
    .step-circle.pending svg { color: var(--opendbpylot-outline-dimmer); }
    .step-circle.running svg { color: var(--opendbpylot-teal); }
    .step-circle.done svg { color: #fff; }
    .step-circle.error svg { color: var(--opendbpylot-accent-negative-default); }

    .step-dot-pending {
      width: 8px; height: 8px; border-radius: 50%;
      background: var(--opendbpylot-outline-dimmer);
    }

    .step-info { flex: 1; min-width: 0; }
    .step-label {
      font-size: 13px; font-weight: 500;
      color: var(--opendbpylot-foreground-dimmest);
      transition: color 0.2s;
    }
    .step-label.running { color: var(--opendbpylot-foreground-default); font-weight: 600; }
    .step-label.done { color: var(--opendbpylot-foreground-default); }
    .step-label.error { color: var(--opendbpylot-accent-negative-default); }

    .step-detail {
      font-size: 11.5px; color: var(--opendbpylot-foreground-dimmest);
      margin-top: 2px; line-height: 1.3;
    }

    /* Vertical connector line between steps */
    .pipeline-connector {
      width: 2px; height: 28px;
      margin-left: 15px;
      border-radius: 2px;
      background: var(--opendbpylot-outline-dimmer);
      transition: background 0.3s;
    }
    .pipeline-connector.active { background: var(--opendbpylot-teal); }

    /* Empty / idle state */
    .activity-idle {
      display: flex; flex-direction: column; align-items: center;
      gap: 10px; padding: 32px 16px; text-align: center;
    }
    .activity-idle-icon {
      width: 40px; height: 40px; border-radius: 50%;
      background: var(--opendbpylot-background-highest);
      display: flex; align-items: center; justify-content: center;
    }
    .activity-idle-icon svg { width: 18px; height: 18px; color: var(--opendbpylot-foreground-dimmest); }
    .activity-idle p { font-size: 12.5px; color: var(--opendbpylot-foreground-dimmest); margin: 0; line-height: 1.4; }

    @media (max-width: 768px) {
      .chat-activity-panel { display: none; }
      .btn-toggle-panel { display: none; }
    }
  `];

  @property() heading = "opendbpylot";
  @property() subtitle = "Chat with your database";
  @property() placeholder = "Ask a question about your data…";
  @property({ reflect: true }) theme = "dark";
  @property({ attribute: "api-base" }) apiBase = "";
  @property({ attribute: "sse-endpoint" }) sseEndpoint = "/api/opendbpylot/v2/chat_sse";
  @property({ attribute: "poll-endpoint" }) pollEndpoint = "/api/opendbpylot/v2/chat_poll";
  @property({ attribute: "ws-endpoint" }) wsEndpoint = "/api/opendbpylot/v2/chat_websocket";
  @property() transport: Transport = "sse";
  @property({ attribute: "custom-headers" }) customHeaders = "";

  @state() private inputValue = "";
  @state() private busy = false;
  @state() private statusKind: "idle" | "working" | "error" | "success" = "idle";
  @state() private statusMsg = "Ready";
  @state() private suggestions: string[] = DEFAULT_EXAMPLES;
  @state() private activityPanelVisible = true;

  // Pipeline stepper
  @state() private steps: Step[] = [];
  @state() private queryStarted = false;

  @query(".messages") private messagesEl!: HTMLElement;

  private client!: OpenDbPylotApiClient;
  private conversationId = "";

  // Fixed pipeline step definitions (in order)
  private readonly PIPELINE: Array<{ id: string; label: string }> = [
    { id: "analyze",  label: "Analyzing question" },
    { id: "generate", label: "Generating SQL" },
    { id: "execute",  label: "Executing query" },
    { id: "format",   label: "Formatting results" },
  ];

  connectedCallback() {
    super.connectedCallback();
    this.client = new OpenDbPylotApiClient({
      baseUrl: this.apiBase,
      sseEndpoint: this.sseEndpoint,
      pollEndpoint: this.pollEndpoint,
      wsEndpoint: this.wsEndpoint,
    });
    if (this.customHeaders) {
      try { this.client.setCustomHeaders(JSON.parse(this.customHeaders)); }
      catch { console.warn("opendbpylot-chat: custom-headers is not valid JSON"); }
    }
    this.conversationId = this.client.generateId();
    this.client.getStarter().then(s => { if (s.length) this.suggestions = s; });
    this.resetPipeline();
  }

  private resetPipeline() {
    this.steps = this.PIPELINE.map(p => ({ ...p, status: "pending" as StepStatus }));
    this.queryStarted = false;
  }

  private setStepStatus(id: string, status: StepStatus, detail?: string) {
    this.steps = this.steps.map(s => s.id === id ? { ...s, status, detail: detail ?? s.detail } : s);
  }

  private activateStep(id: string, detail?: string) {
    // Mark all prior steps done, this one running
    const idx = this.PIPELINE.findIndex(p => p.id === id);
    this.steps = this.steps.map((s, i) => {
      if (i < idx && s.status === "running") return { ...s, status: "done" as StepStatus };
      if (s.id === id) return { ...s, status: "running" as StepStatus, detail: detail ?? s.detail };
      return s;
    });
  }

  private completeStep(id: string) {
    this.setStepStatus(id, "done");
  }

  private completeAllSteps() {
    this.steps = this.steps.map(s =>
      s.status === "running" || s.status === "pending"
        ? { ...s, status: "done" as StepStatus }
        : s
    );
  }

  render() {
    return html`
      <div class="chat-container">
        <div class="header">
          <div class="logo"></div>
          <div class="titles">
            <h1>${this.heading}</h1>
            <p>${this.subtitle}</p>
          </div>
          <div class="status">
            <span class="dot ${this.statusKind}"></span>${this.statusMsg}
          </div>
          <button class="btn-toggle-panel"
            @click=${() => { this.activityPanelVisible = !this.activityPanelVisible; }}>
            ${this.activityPanelVisible ? "Hide" : "Activity"}
          </button>
        </div>

        <div class="chat-body">
          <div class="chat-main">
            <div class="messages"></div>
            <div class="composer">
              <form @submit=${this.onSubmit}>
                <input
                  .value=${this.inputValue}
                  placeholder=${this.placeholder}
                  @input=${(e: Event) => (this.inputValue = (e.target as HTMLInputElement).value)}
                />
                <button class="send" type="submit" ?disabled=${this.busy}>
                  ${this.busy ? "…" : "Ask"}
                </button>
              </form>
            </div>
          </div>

          <div class="chat-activity-panel ${this.activityPanelVisible ? "" : "hidden"}">
            <div class="activity-header">
              <span class="activity-title">Processing</span>
              <button class="activity-close" @click=${() => { this.activityPanelVisible = false; }}>×</button>
            </div>
            <div class="activity-body">
              ${this.renderPipeline()}
            </div>
          </div>
        </div>
      </div>
    `;
  }

  private renderPipeline() {
    if (!this.queryStarted) {
      return html`
        <div class="activity-idle">
          <div class="activity-idle-icon">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round">
              <circle cx="12" cy="12" r="10"/>
              <polyline points="12 6 12 12 16 14"/>
            </svg>
          </div>
          <p>Ask a question to see live processing steps here</p>
        </div>
      `;
    }

    return html`
      <div class="pipeline">
        ${this.steps.map((step, i) => html`
          <div class="pipeline-step">
            <div class="pipeline-step-row">
              <div class="step-circle ${step.status}">
                ${step.status === "pending" ? html`<div class="step-dot-pending"></div>` : nothing}
                ${step.status === "running" ? html`
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round">
                    <circle cx="12" cy="12" r="3"/>
                  </svg>` : nothing}
                ${step.status === "done" ? html`
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round">
                    <polyline points="20 6 9 17 4 12"/>
                  </svg>` : nothing}
                ${step.status === "error" ? html`
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round">
                    <line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/>
                  </svg>` : nothing}
              </div>
              <div class="step-info">
                <div class="step-label ${step.status}">${step.label}</div>
                ${step.detail ? html`<div class="step-detail">${step.detail}</div>` : nothing}
              </div>
            </div>
            ${i < this.steps.length - 1 ? html`
              <div class="pipeline-connector ${step.status === "done" ? "active" : ""}"></div>
            ` : nothing}
          </div>
        `)}
      </div>
    `;
  }

  firstUpdated() {
    this.messagesEl.addEventListener("opendbpylot-action", (e: Event) => {
      const detail = (e as CustomEvent).detail;
      if (detail?.action === "ask" && detail.prompt) this.run(detail.prompt);
    });
    this.renderWelcome();
  }

  private renderWelcome() {
    if (!this.messagesEl) return;
    const w = document.createElement("div");
    w.className = "welcome";
    w.innerHTML = `<h2>Ask a question about your data</h2><p>Try one of these to get started:</p><div class="chips"></div>`;
    const chips = w.querySelector(".chips")!;
    for (const ex of this.suggestions) {
      const c = document.createElement("div");
      c.className = "chip"; c.textContent = ex;
      c.onclick = () => this.run(ex);
      chips.appendChild(c);
    }
    this.messagesEl.appendChild(w);
  }

  private onSubmit(e: Event) { e.preventDefault(); this.run(this.inputValue); }

  private async run(question: string) {
    question = (question || "").trim();
    if (!question || this.busy) return;

    this.busy = true;
    this.inputValue = "";
    this.setStatus("working", "Thinking…");

    // Reset pipeline for new query
    this.resetPipeline();
    this.queryStarted = true;
    this.activateStep("analyze", "Understanding your question");

    await this.updateComplete;
    const welcome = this.messagesEl.querySelector(".welcome");
    if (welcome) welcome.remove();
    this.addUserBubble(question);

    const card = document.createElement("div");
    card.className = "turn bot-card";
    this.messagesEl.appendChild(card);
    const manager = new ComponentManager(card, (c) => this.handleUiState(c));
    this.scrollDown();

    try {
      const gen = this.client.stream(this.transport, {
        message: question,
        conversation_id: this.conversationId,
        request_id: this.client.generateId(),
      });
      for await (const chunk of gen) {
        this.handleChunk(chunk, manager);
      }
    } catch (err) {
      manager.processChunk({ id: "err", type: "notification", data: { level: "error", message: String(err) } });
      this.setStatus("error", "Error");
      this.setStepStatus(this.currentRunningStep() ?? "analyze", "error");
    } finally {
      this.busy = false;
      if (this.statusKind === "working") this.setStatus("idle", "Ready");
      this.completeAllSteps();
      this.scrollDown();
      this.dispatchEvent(new CustomEvent("opendbpylot-turn-complete", {
        bubbles: true, composed: true,
        detail: { conversationId: this.conversationId },
      }));
    }
  }

  private currentRunningStep(): string | null {
    return this.steps.find(s => s.status === "running")?.id ?? null;
  }

  async newConversation(id: string) {
    this.conversationId = id;
    await this.updateComplete;
    if (this.messagesEl) {
      this.messagesEl.innerHTML = "";
      this.renderWelcome();
    }
    this.resetPipeline();
    this.setStatus("idle", "Ready");
  }

  async loadConversation(id: string) {
    this.conversationId = id;
    await this.updateComplete;
    this.messagesEl.innerHTML = "";
    try {
      const res = await fetch(`${this.apiBase}/api/conversations/${encodeURIComponent(id)}`);
      const data = await res.json();
      for (const m of data.messages || []) {
        this.addUserBubble(m.question);
        const card = document.createElement("div");
        card.className = "turn bot-card";
        this.messagesEl.appendChild(card);
        if (m.sql) {
          const pre = document.createElement("pre");
          pre.className = "history-sql"; pre.textContent = m.sql;
          card.appendChild(pre);
          const btn = document.createElement("button");
          btn.className = "history-rerun"; btn.textContent = "↩ Re-run this question";
          btn.onclick = () => this.run(m.question);
          card.appendChild(btn);
        }
      }
    } catch { /* ignore */ }
    this.scrollDown();
  }

  private handleChunk(chunk: ChatStreamChunk, manager: ComponentManager) {
    if (chunk.conversation_id) this.conversationId = chunk.conversation_id;

    if (chunk.rich) {
      const r = chunk.rich;

      if (r.type === "status_bar_update") {
        const d = r.data as any;
        const msg: string = d.message || "";
        const lc = msg.toLowerCase();
        if (lc.includes("generat") || lc.includes("sql")) {
          this.completeStep("analyze");
          this.activateStep("generate", msg);
        } else if (lc.includes("execut") || lc.includes("run") || lc.includes("query")) {
          this.completeStep("generate");
          this.activateStep("execute", msg);
        } else if (lc.includes("format") || lc.includes("chart") || lc.includes("result") || lc.includes("done")) {
          this.completeStep("execute");
          this.activateStep("format", msg);
        }
      }

      if (r.type === "progress_bar") {
        const d = r.data as any;
        const val: number = d.value ?? 0;
        const label: string = (d.label || "").toLowerCase();
        if (val <= 30 || label.includes("analyz")) {
          this.activateStep("analyze", d.label);
        } else if (val <= 60 || label.includes("generat") || label.includes("sql")) {
          this.completeStep("analyze");
          this.activateStep("generate", d.label);
        } else if (val <= 85 || label.includes("run") || label.includes("execut")) {
          this.completeStep("generate");
          this.activateStep("execute", d.label);
        } else {
          this.completeStep("execute");
          this.activateStep("format", d.label);
        }
      }

      // When we get data (dataframe/chart) — execution is happening
      if (r.type === "dataframe" || r.type === "chart") {
        this.completeStep("generate");
        this.completeStep("execute");
        this.activateStep("format");
      }

      // Text chunks mean SQL was generated
      if (r.type === "text") {
        this.completeStep("analyze");
        if (this.steps.find(s => s.id === "generate")?.status === "pending") {
          this.activateStep("generate");
        }
      }

      manager.processChunk(r);
    } else if (chunk.simple?.text) {
      this.completeStep("analyze");
      if (this.steps.find(s => s.id === "generate")?.status === "pending") {
        this.activateStep("generate");
      }
      manager.processChunk({ id: `s-${Date.now()}`, type: "text", data: { text: chunk.simple.text } });
    }
    this.scrollDown();
  }

  private handleUiState(c: RichComponent) {
    if (c.type === "status_bar_update") {
      this.setStatus((c.data.status as any) || "idle", c.data.message || "");
    } else if (c.type === "chat_input_update") {
      this.busy = !!c.data.disabled;
      if (c.data.placeholder) this.placeholder = c.data.placeholder;
    }
  }

  private addUserBubble(text: string) {
    const el = document.createElement("div");
    el.className = "msg-user"; el.textContent = text;
    this.messagesEl.appendChild(el);
  }

  private setStatus(kind: "idle" | "working" | "error" | "success", msg: string) {
    this.statusKind = kind;
    this.statusMsg = msg || (kind === "idle" ? "Ready" : "");
  }

  private scrollDown() {
    if (this.messagesEl) this.messagesEl.scrollTop = this.messagesEl.scrollHeight;
  }
}
