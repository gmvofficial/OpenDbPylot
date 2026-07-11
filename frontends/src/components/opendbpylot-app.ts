import { LitElement, html, css, nothing } from "lit";
import { customElement, state } from "lit/decorators.js";
import { tokens } from "../styles/design-tokens";
import "./opendbpylot-chat";

interface Provider { id: string; label: string; needs_key: boolean; }
interface AppSettings { provider: string; model: string; db_kind: string; db_path: string; db_connection_string: string; key_set: boolean; ready: boolean; duckdb_available?: boolean; }
interface Conversation { id: string; title: string; }
type View = "chat" | "settings" | "train";

@customElement("opendbpylot-app")
export class OpenDbPylotApp extends LitElement {
  static styles = [tokens, css`
    *, *::before, *::after { box-sizing: border-box; }

    :host {
      display: flex;
      height: 100vh;
      overflow: hidden;
      background: var(--opendbpylot-background-root);
      color: var(--opendbpylot-foreground-default);
      font-family: var(--opendbpylot-font-family-default);
      font-size: 14px;
    }

    /* ─── Sidebar ─── */
    .sidebar {
      width: 248px;
      flex-shrink: 0;
      display: flex;
      flex-direction: column;
      background: var(--opendbpylot-background-higher);
      border-right: 1px solid var(--opendbpylot-outline-dimmer);
      transition: transform 0.22s ease;
    }

    .sidebar-header {
      padding: 16px 14px;
      border-bottom: 1px solid var(--opendbpylot-outline-dimmer);
      display: flex; align-items: center; gap: 10px; flex-shrink: 0;
    }

    .logo-mark {
      width: 30px; height: 30px; border-radius: 8px;
      background: linear-gradient(135deg, var(--opendbpylot-teal), var(--opendbpylot-navy));
      display: flex; align-items: center; justify-content: center; flex-shrink: 0;
    }
    .logo-mark svg { width: 16px; height: 16px; fill: #fff; }
    .logo-text { font-size: 15px; font-weight: 700; letter-spacing: -0.3px; }
    .logo-badge {
      margin-left: auto; font-size: 10px; font-weight: 600;
      padding: 2px 7px; border-radius: 999px; letter-spacing: 0.05em;
      background: rgba(21,168,168,.15); color: var(--opendbpylot-teal);
    }

    .sidebar-actions { padding: 10px 10px 6px; }

    .btn-new-chat {
      width: 100%; padding: 9px 14px;
      background: var(--opendbpylot-teal); color: #fff;
      font-size: 13px; font-weight: 600;
      font-family: var(--opendbpylot-font-family-default);
      border: none; border-radius: 8px; cursor: pointer;
      display: flex; align-items: center; gap: 8px;
      transition: opacity 0.15s;
    }
    .btn-new-chat:hover { opacity: 0.88; }
    .btn-new-chat svg { width: 14px; height: 14px; flex-shrink: 0; }

    .section-label {
      padding: 12px 14px 4px;
      font-size: 10px; font-weight: 700;
      text-transform: uppercase; letter-spacing: 0.1em;
      color: var(--opendbpylot-foreground-dimmest);
    }

    .conv-list { flex: 1; overflow-y: auto; padding: 2px 6px 4px; }
    .conv-list::-webkit-scrollbar { width: 3px; }
    .conv-list::-webkit-scrollbar-thumb { background: var(--opendbpylot-outline-dimmer); border-radius: 3px; }

    .conv-item {
      padding: 8px 10px; border-radius: 6px; cursor: pointer;
      font-size: 13px; color: var(--opendbpylot-foreground-dimmest);
      white-space: nowrap; overflow: hidden; text-overflow: ellipsis;
      transition: background 0.1s, color 0.1s;
      display: flex; align-items: center; gap: 8px;
    }
    .conv-item::before {
      content: ''; width: 6px; height: 6px; border-radius: 50%;
      background: var(--opendbpylot-outline-dimmer); flex-shrink: 0;
    }
    .conv-item:hover { background: var(--opendbpylot-background-highest); color: var(--opendbpylot-foreground-default); }
    .conv-item.active { background: rgba(21,168,168,.1); color: var(--opendbpylot-foreground-default); }
    .conv-item.active::before { background: var(--opendbpylot-teal); }

    .conv-title { flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
    .conv-del {
      flex-shrink: 0; opacity: 0; border: none; background: transparent; cursor: pointer;
      color: var(--opendbpylot-foreground-dimmest); padding: 2px; border-radius: 4px;
      display: flex; align-items: center; transition: opacity .1s, color .1s, background .1s;
    }
    .conv-del svg { width: 14px; height: 14px; }
    .conv-item:hover .conv-del { opacity: 0.65; }
    .conv-del:hover { opacity: 1; color: var(--opendbpylot-orange); background: rgba(254,93,38,.14); }

    /* Save button: disabled + saved state (spinner reuses the existing .spin) */
    .btn-primary:disabled { opacity: 0.75; cursor: default; }
    .btn-primary.btn-saved { background: var(--opendbpylot-teal); }

    .empty-conv {
      padding: 12px 14px; font-size: 12px;
      color: var(--opendbpylot-foreground-dimmest); font-style: italic;
    }

    /* ─── Sidebar footer nav ─── */
    .sidebar-footer { border-top: 1px solid var(--opendbpylot-outline-dimmer); padding: 6px; flex-shrink: 0; }

    .nav-item {
      display: flex; align-items: center; gap: 10px;
      padding: 9px 10px; border-radius: 6px; cursor: pointer;
      font-size: 13px; font-family: var(--opendbpylot-font-family-default);
      color: var(--opendbpylot-foreground-dimmest);
      border: none; background: transparent; width: 100%; text-align: left;
      transition: background 0.1s, color 0.1s;
    }
    .nav-item:hover { background: var(--opendbpylot-background-highest); color: var(--opendbpylot-foreground-default); }
    .nav-item.active { background: rgba(21,168,168,.1); color: var(--opendbpylot-teal); }
    .nav-item svg { width: 16px; height: 16px; flex-shrink: 0; }

    .connection-badge {
      margin-left: auto;
      display: inline-flex; align-items: center; gap: 5px;
      font-size: 11px; font-weight: 500; padding: 2px 8px; border-radius: 999px;
    }
    .connection-badge.ok { background: rgba(21,168,168,.12); color: var(--opendbpylot-teal); }
    .connection-badge.bad { background: rgba(254,93,38,.12); color: var(--opendbpylot-orange); }
    .connection-badge::before { content: ''; width: 5px; height: 5px; border-radius: 50%; background: currentColor; }

    /* ─── Main ─── */
    .main { flex: 1; min-width: 0; display: flex; flex-direction: column; }
    opendbpylot-chat { display: block; flex: 1; min-height: 0; }

    /* ─── Mobile header ─── */
    .mobile-header {
      display: none; align-items: center; gap: 12px; padding: 12px 16px;
      background: var(--opendbpylot-background-higher);
      border-bottom: 1px solid var(--opendbpylot-outline-dimmer); flex-shrink: 0;
    }
    .btn-menu {
      border: 1px solid var(--opendbpylot-outline-dimmer);
      background: var(--opendbpylot-background-highest);
      border-radius: 6px; padding: 6px 10px; cursor: pointer;
      color: var(--opendbpylot-foreground-default);
      font-family: var(--opendbpylot-font-family-default); font-size: 13px;
      display: flex; align-items: center; gap: 6px;
    }
    .mobile-overlay {
      display: none; position: fixed; inset: 0; z-index: 99;
      background: rgba(0,0,0,0.5);
    }

    @media (max-width: 768px) {
      .sidebar {
        position: fixed; top: 0; left: 0; bottom: 0; z-index: 100;
        transform: translateX(-100%); box-shadow: var(--opendbpylot-shadow-2xl);
      }
      .sidebar.open { transform: translateX(0); }
      .mobile-header { display: flex; }
      .mobile-overlay.active { display: block; }
    }

    /* ─── Full-page views (Settings / Train) ─── */
    .page-view {
      flex: 1; overflow-y: auto;
      background: var(--opendbpylot-background-root);
      display: flex; flex-direction: column;
    }

    .page-topbar {
      display: flex; align-items: center;
      background: var(--opendbpylot-background-higher);
      border-bottom: 1px solid var(--opendbpylot-outline-dimmer);
      padding: 0 32px; height: 56px; flex-shrink: 0;
    }

    .btn-back {
      display: flex; align-items: center; gap: 7px;
      padding: 7px 12px; border-radius: 6px;
      border: 1px solid var(--opendbpylot-outline-dimmer);
      background: transparent;
      color: var(--opendbpylot-foreground-dimmest);
      font-size: 13px; font-weight: 500;
      font-family: var(--opendbpylot-font-family-default);
      cursor: pointer; transition: all 0.12s; margin-right: 16px;
    }
    .btn-back:hover { color: var(--opendbpylot-foreground-default); border-color: var(--opendbpylot-foreground-dimmest); }
    .btn-back svg { width: 14px; height: 14px; }

    .topbar-divider { width: 1px; height: 20px; background: var(--opendbpylot-outline-dimmer); margin-right: 16px; }
    .topbar-title { font-size: 15px; font-weight: 600; }

    .page-hero {
      padding: 40px 32px 32px;
      border-bottom: 1px solid var(--opendbpylot-outline-dimmer);
      background: var(--opendbpylot-background-higher);
    }
    .page-hero-label {
      font-size: 11px; font-weight: 700; text-transform: uppercase;
      letter-spacing: 0.1em; color: var(--opendbpylot-teal); margin-bottom: 8px;
    }
    .page-hero-title { font-size: 26px; font-weight: 700; margin: 0 0 6px; letter-spacing: -0.4px; }
    .page-hero-sub { font-size: 14px; color: var(--opendbpylot-foreground-dimmest); margin: 0; line-height: 1.5; }

    .page-content {
      padding: 32px; display: flex; flex-direction: column;
      width: 100%;
    }

    /* ─── Settings cards ─── */
    .settings-card {
      background: var(--opendbpylot-background-higher);
      border: 1px solid var(--opendbpylot-outline-dimmer);
      border-radius: 12px; overflow: hidden; margin-bottom: 20px;
    }
    .settings-card-header {
      padding: 16px 20px;
      border-bottom: 1px solid var(--opendbpylot-outline-dimmer);
      background: var(--opendbpylot-background-highest);
      display: flex; align-items: center; gap: 12px;
    }
    .settings-card-icon {
      width: 34px; height: 34px; border-radius: 8px;
      display: flex; align-items: center; justify-content: center; flex-shrink: 0;
    }
    .settings-card-icon.teal { background: rgba(21,168,168,.15); color: var(--opendbpylot-teal); }
    .settings-card-icon.magenta { background: rgba(191,19,99,.12); color: var(--opendbpylot-magenta); }
    .settings-card-icon svg { width: 17px; height: 17px; }
    .settings-card-title { font-size: 14px; font-weight: 600; margin: 0; }
    .settings-card-desc { font-size: 12px; color: var(--opendbpylot-foreground-dimmest); margin: 2px 0 0; }
    .settings-card-body { padding: 20px; display: flex; flex-direction: column; gap: 16px; }

    /* ─── Form elements ─── */
    .field { display: flex; flex-direction: column; gap: 6px; }
    .field-row { display: flex; gap: 14px; }
    .field-row .field { flex: 1; }

    .field-label {
      font-size: 12.5px; font-weight: 600;
      color: var(--opendbpylot-foreground-default);
      display: flex; align-items: center; gap: 8px;
    }
    .field-label-optional { font-weight: 400; font-size: 11px; color: var(--opendbpylot-foreground-dimmest); }
    .field-hint { font-size: 11.5px; color: var(--opendbpylot-foreground-dimmest); line-height: 1.4; margin-top: 2px; }
    .field-label svg { width: 13px; height: 13px; flex-shrink: 0; }

    input, select, textarea {
      background: var(--opendbpylot-background-root);
      border: 1px solid var(--opendbpylot-outline-dimmer);
      border-radius: 8px;
      color: var(--opendbpylot-foreground-default);
      padding: 10px 12px;
      font-size: 13.5px; font-family: var(--opendbpylot-font-family-default);
      outline: none; width: 100%;
      transition: border-color 0.15s, box-shadow 0.15s;
    }
    input:focus, select:focus, textarea:focus {
      border-color: var(--opendbpylot-teal);
      box-shadow: 0 0 0 3px rgba(21,168,168,.15);
    }
    select {
      appearance: none;
      background-image: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='12' height='12' viewBox='0 0 24 24' fill='none' stroke='%2394a3b8' stroke-width='2'%3E%3Cpath d='m6 9 6 6 6-6'/%3E%3C/svg%3E");
      background-repeat: no-repeat; background-position: right 12px center; padding-right: 32px;
    }
    textarea { resize: vertical; min-height: 100px; line-height: 1.6; }

    .key-status {
      display: inline-flex; align-items: center; gap: 5px;
      font-size: 11px; font-weight: 600; padding: 3px 9px; border-radius: 999px;
    }
    .key-status::before { content: ''; width: 5px; height: 5px; border-radius: 50%; background: currentColor; }
    .key-status.set { background: rgba(21,168,168,.12); color: var(--opendbpylot-teal); }
    .key-status.unset { background: rgba(254,93,38,.12); color: var(--opendbpylot-orange); }

    .form-actions { display: flex; align-items: center; gap: 14px; padding-top: 4px; }

    .btn {
      padding: 10px 20px; border-radius: 8px;
      font-size: 13.5px; font-weight: 600; cursor: pointer;
      font-family: var(--opendbpylot-font-family-default);
      border: 1px solid var(--opendbpylot-outline-dimmer);
      background: var(--opendbpylot-background-highest);
      color: var(--opendbpylot-foreground-default);
      display: inline-flex; align-items: center; gap: 8px;
      transition: all 0.15s; white-space: nowrap;
    }
    .btn:hover { border-color: var(--opendbpylot-foreground-dimmest); }
    .btn:disabled { opacity: 0.5; cursor: not-allowed; }
    .btn svg { width: 15px; height: 15px; }

    .btn-primary { background: var(--opendbpylot-teal); border-color: var(--opendbpylot-teal); color: #fff; }
    .btn-primary:hover { opacity: 0.88; border-color: transparent; }

    .btn-ghost {
      background: transparent; border-color: transparent;
      color: var(--opendbpylot-foreground-dimmest); padding: 10px 14px;
    }
    .btn-ghost:hover { color: var(--opendbpylot-foreground-default); background: var(--opendbpylot-background-highest); border-color: var(--opendbpylot-outline-dimmer); }

    .toast-msg { font-size: 13px; display: flex; align-items: center; gap: 6px; }
    .toast-msg.ok { color: var(--opendbpylot-teal); }
    .toast-msg.err { color: var(--opendbpylot-orange); }
    .toast-msg svg { width: 14px; height: 14px; flex-shrink: 0; }

    /* ─── Database type selector ─── */
    .db-type-grid { display: grid; grid-template-columns: repeat(3, 1fr); gap: 10px; }
    .db-type-btn {
      display: flex; flex-direction: column; align-items: flex-start; gap: 3px;
      padding: 12px 14px; border-radius: 10px;
      border: 1px solid var(--opendbpylot-outline-dimmer);
      background: var(--opendbpylot-background-root);
      cursor: pointer; text-align: left;
      transition: border-color 0.15s, background 0.15s;
      font-family: var(--opendbpylot-font-family-default);
    }
    .db-type-btn:hover { border-color: var(--opendbpylot-foreground-dimmest); }
    .db-type-btn.active {
      border-color: var(--opendbpylot-teal);
      background: rgba(21,168,168,.08);
    }
    .db-type-name { font-size: 13.5px; font-weight: 600; color: var(--opendbpylot-foreground-default); }
    .db-type-hint { font-size: 11px; color: var(--opendbpylot-foreground-dimmest); }
    .db-type-btn.active .db-type-name { color: var(--opendbpylot-teal); }

    @media (max-width: 480px) {
      .db-type-grid { grid-template-columns: 1fr; }
    }

    /* ─── Train page ─── */
    .train-intro {
      padding: 18px 20px;
      background: rgba(21,168,168,.06);
      border: 1px solid rgba(21,168,168,.2);
      border-radius: 10px; margin-bottom: 24px;
      font-size: 13.5px; line-height: 1.6;
    }
    .train-intro strong { color: var(--opendbpylot-teal); }

    .train-section {
      background: var(--opendbpylot-background-higher);
      border: 1px solid var(--opendbpylot-outline-dimmer);
      border-radius: 12px; overflow: hidden; margin-bottom: 16px;
    }
    .train-section-header {
      padding: 16px 20px;
      border-bottom: 1px solid var(--opendbpylot-outline-dimmer);
      display: flex; align-items: flex-start; gap: 12px;
    }
    .train-section-num {
      width: 26px; height: 26px; border-radius: 50%;
      background: rgba(21,168,168,.15); color: var(--opendbpylot-teal);
      font-size: 12px; font-weight: 700;
      display: flex; align-items: center; justify-content: center;
      flex-shrink: 0; margin-top: 1px;
    }
    .train-section-info { flex: 1; }
    .train-section-title { font-size: 14px; font-weight: 600; margin: 0 0 3px; }
    .train-section-desc { font-size: 12.5px; color: var(--opendbpylot-foreground-dimmest); margin: 0; line-height: 1.4; }
    .train-section-body { padding: 20px; display: flex; flex-direction: column; gap: 14px; }

    .schema-action-box {
      display: flex; align-items: center; gap: 16px;
      padding: 16px;
      background: var(--opendbpylot-background-highest);
      border: 1px dashed var(--opendbpylot-outline-dimmer);
      border-radius: 10px;
    }
    .schema-action-icon {
      width: 44px; height: 44px; border-radius: 10px;
      background: rgba(21,168,168,.12); color: var(--opendbpylot-teal);
      display: flex; align-items: center; justify-content: center; flex-shrink: 0;
    }
    .schema-action-icon svg { width: 22px; height: 22px; }
    .schema-action-info { flex: 1; min-width: 0; }
    .schema-action-info strong { display: block; font-size: 13.5px; font-weight: 600; margin-bottom: 3px; }
    .schema-action-info span { font-size: 12px; color: var(--opendbpylot-foreground-dimmest); }

    @keyframes spin { to { transform: rotate(360deg); } }
    .spin { animation: spin 0.9s linear infinite; }

    @media (max-width: 768px) {
      .page-topbar { padding: 0 16px; }
      .page-hero { padding: 24px 16px 20px; }
      .page-content { padding: 20px 16px; }
      .field-row { flex-direction: column; }
      .schema-action-box { flex-direction: column; align-items: flex-start; }
    }
  `];

  @state() private providers: Provider[] = [];
  @state() private settings: AppSettings = { provider: "openai", model: "", db_kind: "sqlite", db_path: "", db_connection_string: "", key_set: false, ready: false };
  @state() private conversations: Conversation[] = [];
  @state() private activeConv = "";
  @state() private currentView: View = "chat";
  @state() private sidebarOpen = false;

  @state() private fProvider = "openai";
  @state() private fModel = "";
  @state() private fDbKind = "sqlite";
  @state() private fDbPath = "";
  @state() private fDbConnStr = "";
  @state() private fApiKey = "";
  @state() private settingsToast = "";
  @state() private settingsToastErr = false;
  // Save-button state machine: idle → saving → saved (or error). Resets to idle
  // whenever the user edits any field, so the button re-invites "Save & connect".
  @state() private saveState: "idle" | "saving" | "saved" | "error" = "idle";

  @state() private schemaToast = "";
  @state() private schemaLoading = false;
  @state() private trainDoc = "";
  @state() private docToast = "";
  @state() private trainDdl = "";
  @state() private ddlToast = "";
  @state() private trainQ = "";
  @state() private trainSql = "";
  @state() private sqlToast = "";

  connectedCallback() {
    super.connectedCallback();
    this.boot();
    this.addEventListener("opendbpylot-turn-complete", (e: Event) => {
      const d = (e as CustomEvent).detail;
      if (d?.conversationId) this.activeConv = d.conversationId;
      this.refreshConversations();
    });
  }

  private async jget(url: string) { return (await fetch(url)).json(); }
  private async jdelete(url: string) { return (await fetch(url, { method: "DELETE" })).json(); }
  private async jpost(url: string, body?: unknown) {
    return (await fetch(url, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body ?? {}),
    })).json();
  }

  private async boot() {
    const { providers } = await this.jget("/api/providers");
    this.providers = providers;
    await this.loadSettings();
    await this.refreshConversations();
    await this.newChat();
  }

  private async loadSettings() {
    const s: AppSettings = await this.jget("/api/settings");
    this.settings = s;
    this.fProvider = s.provider;
    this.fModel = "";
    this.fDbKind = s.db_kind || "sqlite";
    this.fDbPath = s.db_path || "";
    this.fDbConnStr = s.db_connection_string || "";
    if (!s.ready) this.currentView = "settings";
  }

  private async refreshConversations() {
    const { conversations } = await this.jget("/api/conversations");
    this.conversations = conversations || [];
  }

  private async openConversation(id: string) {
    this.activeConv = id;
    this.currentView = "chat";
    this.sidebarOpen = false;
    await this.updateComplete;
    await (this.shadowRoot?.querySelector("opendbpylot-chat") as any)?.loadConversation(id);
    this.refreshConversations();
  }

  private async newChat() {
    const { id } = await this.jpost("/api/conversations");
    this.activeConv = id;
    // Always return to the chat view first — otherwise <opendbpylot-chat> isn't
    // rendered (e.g. when on Settings/Train) and there's nothing to reset.
    this.currentView = "chat";
    this.sidebarOpen = false;
    await this.updateComplete;
    await (this.shadowRoot?.querySelector("opendbpylot-chat") as any)?.newConversation(id);
    this.refreshConversations();
  }

  private async deleteConversation(id: string, e: Event) {
    e.stopPropagation(); // don't also open the conversation
    if (!confirm("Delete this conversation? This can't be undone.")) return;
    await this.jdelete(`/api/conversations/${encodeURIComponent(id)}`);
    // If we deleted the one we're viewing, start a fresh chat; otherwise just refresh.
    if (id === this.activeConv) {
      await this.newChat();
    } else {
      await this.refreshConversations();
    }
  }

  /// Called on any settings-field edit: a change means the current save is stale,
  /// so reset the button to invite "Save & connect" again (e.g. after switching DB).
  private markSettingsDirty() {
    if (this.saveState !== "idle") this.saveState = "idle";
  }

  private async saveSettings() {
    if (this.saveState === "saving") return; // ignore double-clicks
    this.saveState = "saving";
    this.settingsToast = "";
    try {
      const s = await this.jpost("/api/settings", {
        provider: this.fProvider,
        model: this.fModel.trim(),
        db_kind: this.fDbKind,
        db_path: this.fDbPath.trim(),
        db_connection_string: this.fDbConnStr.trim(),
        api_key: this.fApiKey.trim(),
      });
      this.fApiKey = "";
      this.settings = s;
      if (!s.ready) {
        this.saveState = "error";
        this.settingsToast = "Saved — API key still required";
        this.settingsToastErr = true;
      } else if (s.db_error) {
        this.saveState = "error";
        this.settingsToast = `Saved, but couldn't reach the database: ${s.db_error}`;
        this.settingsToastErr = true;
      } else {
        this.saveState = "saved";
        this.settingsToast = s.tables_imported > 0
          ? `Connected — learned ${s.tables_imported} table(s) from your database`
          : "Saved & connected";
        this.settingsToastErr = false;
      }
    } catch (err) {
      this.saveState = "error";
      this.settingsToast = "Couldn't save settings — is the server running?";
      this.settingsToastErr = true;
    }
    setTimeout(() => { this.settingsToast = ""; }, 5000);
  }

  /// Button label + icon derived from saveState.
  private saveButtonContent() {
    switch (this.saveState) {
      case "saving": return html`${this.iSpin()} Connecting…`;
      case "saved":  return html`${this.iCheck()} Saved &amp; connected`;
      default:       return html`${this.iCheck()} Save &amp; connect`;
    }
  }

  private async learnSchema() {
    this.schemaLoading = true;
    const r = await this.jpost("/api/learn_schema");
    this.schemaToast = r.ok ? `Imported ${r.tables_learned} table(s)` : r.error || "Failed";
    this.schemaLoading = false;
    setTimeout(() => { this.schemaToast = ""; }, 4000);
  }

  private async addDoc() {
    if (!this.trainDoc.trim()) return;
    const r = await this.jpost("/api/train", { kind: "doc", text: this.trainDoc.trim() });
    this.trainDoc = ""; this.docToast = r.ok ? "Documentation added" : "Failed";
    setTimeout(() => { this.docToast = ""; }, 3000);
  }

  private async addDdl() {
    if (!this.trainDdl.trim()) return;
    const r = await this.jpost("/api/train", { kind: "ddl", text: this.trainDdl.trim() });
    this.trainDdl = ""; this.ddlToast = r.ok ? "Table definition added" : "Failed";
    setTimeout(() => { this.ddlToast = ""; }, 3000);
  }

  private async addSql() {
    if (!this.trainQ.trim() || !this.trainSql.trim()) return;
    const r = await this.jpost("/api/train", { kind: "sql", question: this.trainQ.trim(), sql: this.trainSql.trim() });
    this.trainQ = ""; this.trainSql = "";
    this.sqlToast = r.ok ? "Example added" : "Failed";
    setTimeout(() => { this.sqlToast = ""; }, 3000);
  }

  // ── Icons ──
  private iPlus() { return html`<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round"><line x1="12" y1="5" x2="12" y2="19"/><line x1="5" y1="12" x2="19" y2="12"/></svg>`; }
  private iBook() { return html`<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><path d="M2 3h6a4 4 0 0 1 4 4v14a3 3 0 0 0-3-3H2z"/><path d="M22 3h-6a4 4 0 0 0-4 4v14a3 3 0 0 1 3-3h7z"/></svg>`; }
  private iGear() { return html`<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"/></svg>`; }
  private iBack() { return html`<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="m15 18-6-6 6-6"/></svg>`; }
  private iDB() { return html`<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><ellipse cx="12" cy="5" rx="9" ry="3"/><path d="M21 12c0 1.66-4 3-9 3s-9-1.34-9-3"/><path d="M3 5v14c0 1.66 4 3 9 3s9-1.34 9-3V5"/></svg>`; }
  private iLLM() { return html`<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><rect x="3" y="3" width="18" height="18" rx="3"/><path d="M9 9h6M9 12h6M9 15h4"/></svg>`; }
  private iCheck() { return html`<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round"><polyline points="20 6 9 17 4 12"/></svg>`; }
  private iTrash() { return html`<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><polyline points="3 6 5 6 21 6"/><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/></svg>`; }
  private iWarn() { return html`<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><circle cx="12" cy="12" r="10"/><line x1="12" y1="8" x2="12" y2="12"/><line x1="12" y1="16" x2="12.01" y2="16"/></svg>`; }
  private iDoc() { return html`<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><polyline points="14 2 14 8 20 8"/><line x1="16" y1="13" x2="8" y2="13"/><line x1="16" y1="17" x2="8" y2="17"/></svg>`; }
  private iCode() { return html`<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><polyline points="16 18 22 12 16 6"/><polyline points="8 6 2 12 8 18"/></svg>`; }
  private iMsg() { return html`<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"/></svg>`; }
  private iMenu() { return html`<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><line x1="3" y1="6" x2="21" y2="6"/><line x1="3" y1="12" x2="21" y2="12"/><line x1="3" y1="18" x2="21" y2="18"/></svg>`; }
  private iSpin() { return html`<svg class="spin" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round"><path d="M21 12a9 9 0 1 1-6.22-8.56"/></svg>`; }

  private toast(msg: string, isErr: boolean) {
    if (!msg) return nothing;
    return html`<span class="toast-msg ${isErr ? "err" : "ok"}">${isErr ? this.iWarn() : this.iCheck()} ${msg}</span>`;
  }

  // ── Settings page ──
  private renderSettings() {
    const needsKey = this.providers.find(p => p.id === this.fProvider)?.needs_key ?? false;
    return html`
      <div class="page-view">
        <div class="page-topbar">
          <button class="btn-back" @click=${() => { this.currentView = "chat"; }}>${this.iBack()} Back</button>
          <div class="topbar-divider"></div>
          <span class="topbar-title">Settings</span>
        </div>

        <div class="page-hero">
          <div class="page-hero-label">Configuration</div>
          <h1 class="page-hero-title">Settings</h1>
          <p class="page-hero-sub">Connect opendbpylot to your AI provider and SQLite database. API keys are encrypted at rest on this machine and never returned to the browser.</p>
        </div>

        <div class="page-content">
          <div class="settings-card">
            <div class="settings-card-header">
              <div class="settings-card-icon teal">${this.iLLM()}</div>
              <div>
                <p class="settings-card-title">Language Model</p>
                <p class="settings-card-desc">Choose which AI provider and model powers SQL generation</p>
              </div>
            </div>
            <div class="settings-card-body">
              <div class="field-row">
                <div class="field">
                  <label class="field-label">Provider</label>
                  <select .value=${this.fProvider}
                    @change=${(e: Event) => { this.fProvider = (e.target as HTMLSelectElement).value; this.markSettingsDirty(); }}>
                    ${this.providers.map(p => html`<option value="${p.id}">${p.label}</option>`)}
                  </select>
                </div>
                <div class="field">
                  <label class="field-label">Model <span class="field-label-optional">(optional)</span></label>
                  <input type="text"
                    placeholder=${this.settings.model || "Provider default"}
                    .value=${this.fModel}
                    @input=${(e: Event) => { this.fModel = (e.target as HTMLInputElement).value; this.markSettingsDirty(); }} />
                  <span class="field-hint">Leave blank to use the recommended model</span>
                </div>
              </div>

              ${needsKey ? html`
                <div class="field">
                  <label class="field-label">
                    API Key
                    <span class="key-status ${this.settings.key_set ? "set" : "unset"}">
                      ${this.settings.key_set ? "Saved" : "Not set"}
                    </span>
                  </label>
                  <input type="password"
                    placeholder=${this.settings.key_set ? "Enter a new key to replace the saved one" : "Paste your API key"}
                    .value=${this.fApiKey}
                    @input=${(e: Event) => { this.fApiKey = (e.target as HTMLInputElement).value; this.markSettingsDirty(); }} />
                </div>` : nothing}
            </div>
          </div>

          <div class="settings-card">
            <div class="settings-card-header">
              <div class="settings-card-icon magenta">${this.iDB()}</div>
              <div>
                <p class="settings-card-title">Database</p>
                <p class="settings-card-desc">Connect to a local SQLite file or a remote PostgreSQL / MySQL database</p>
              </div>
            </div>
            <div class="settings-card-body">
              <!-- Database type selector -->
              <div class="field">
                <label class="field-label">Database type</label>
                <div class="db-type-grid">
                  ${[
                    { id: "sqlite",   label: "SQLite",     hint: "Local file" },
                    { id: "postgres", label: "PostgreSQL", hint: "Remote / cloud" },
                    { id: "mysql",    label: "MySQL",      hint: "Remote / cloud" },
                    // Only offered when the server binary was compiled with the
                    // `duckdb` feature (the backend advertises availability).
                    ...(this.settings.duckdb_available
                      ? [{ id: "duckdb", label: "DuckDB", hint: "Local file / CSV / Parquet" }]
                      : []),
                  ].map(db => html`
                    <button
                      class="db-type-btn ${this.fDbKind === db.id ? "active" : ""}"
                      @click=${() => { this.fDbKind = db.id; this.markSettingsDirty(); }}>
                      <span class="db-type-name">${db.label}</span>
                      <span class="db-type-hint">${db.hint}</span>
                    </button>
                  `)}
                </div>
              </div>

              <!-- SQLite: file path -->
              ${this.fDbKind === "sqlite" ? html`
                <div class="field">
                  <label class="field-label">File path</label>
                  <input type="text" placeholder="demo.db"
                    .value=${this.fDbPath}
                    @input=${(e: Event) => { this.fDbPath = (e.target as HTMLInputElement).value; this.markSettingsDirty(); }} />
                  <span class="field-hint">
                    Path relative to the server's working directory.
                    Example: <code style="font-family:var(--opendbpylot-font-family-mono);font-size:11px;background:rgba(255,255,255,.07);padding:1px 6px;border-radius:4px;">data/production.db</code>
                  </span>
                </div>
              ` : nothing}

              <!-- DuckDB: file path (or :memory:) -->
              ${this.fDbKind === "duckdb" ? html`
                <div class="field">
                  <label class="field-label">File path</label>
                  <input type="text" placeholder="data.duckdb  (or  :memory:)"
                    .value=${this.fDbPath}
                    @input=${(e: Event) => { this.fDbPath = (e.target as HTMLInputElement).value; this.markSettingsDirty(); }} />
                  <span class="field-hint">
                    Path to a <code style="font-family:var(--opendbpylot-font-family-mono);font-size:11px;background:rgba(255,255,255,.07);padding:1px 6px;border-radius:4px;">.duckdb</code> file, or
                    <code style="font-family:var(--opendbpylot-font-family-mono);font-size:11px;background:rgba(255,255,255,.07);padding:1px 6px;border-radius:4px;">:memory:</code> for a scratch database.
                    Queries can also read local files directly, e.g.
                    <code style="font-family:var(--opendbpylot-font-family-mono);font-size:11px;background:rgba(255,255,255,.07);padding:1px 6px;border-radius:4px;">SELECT * FROM 'sales.csv'</code>.
                  </span>
                </div>
              ` : nothing}

              <!-- PostgreSQL: connection URL -->
              ${this.fDbKind === "postgres" ? html`
                <div class="field">
                  <label class="field-label">Connection URL</label>
                  <input type="text"
                    placeholder="postgresql://user:password@host:5432/database"
                    .value=${this.fDbConnStr}
                    @input=${(e: Event) => { this.fDbConnStr = (e.target as HTMLInputElement).value; this.markSettingsDirty(); }} />
                  <span class="field-hint">
                    Format: <code style="font-family:var(--opendbpylot-font-family-mono);font-size:11px;background:rgba(255,255,255,.07);padding:1px 6px;border-radius:4px;">postgresql://user:password@host:5432/dbname</code>
                    — also accepts <code style="font-family:var(--opendbpylot-font-family-mono);font-size:11px;background:rgba(255,255,255,.07);padding:1px 6px;border-radius:4px;">postgres://</code> prefix.
                    Connections to remote hosts work over SSL automatically.
                  </span>
                </div>
              ` : nothing}

              <!-- MySQL: connection URL -->
              ${this.fDbKind === "mysql" ? html`
                <div class="field">
                  <label class="field-label">Connection URL</label>
                  <input type="text"
                    placeholder="mysql://user:password@host:3306/database"
                    .value=${this.fDbConnStr}
                    @input=${(e: Event) => { this.fDbConnStr = (e.target as HTMLInputElement).value; this.markSettingsDirty(); }} />
                  <span class="field-hint">
                    Format: <code style="font-family:var(--opendbpylot-font-family-mono);font-size:11px;background:rgba(255,255,255,.07);padding:1px 6px;border-radius:4px;">mysql://user:password@host:3306/dbname</code>.
                    MariaDB is also supported using the same URL format.
                  </span>
                </div>
              ` : nothing}
            </div>
          </div>

          <div class="form-actions">
            <button class="btn btn-primary ${this.saveState === "saved" ? "btn-saved" : ""}"
              ?disabled=${this.saveState === "saving"}
              @click=${this.saveSettings}>${this.saveButtonContent()}</button>
            <button class="btn btn-ghost" @click=${() => { this.currentView = "chat"; }}>Cancel</button>
            ${this.toast(this.settingsToast, this.settingsToastErr)}
          </div>
        </div>
      </div>
    `;
  }

  // ── Train page ──
  private renderTrain() {
    return html`
      <div class="page-view">
        <div class="page-topbar">
          <button class="btn-back" @click=${() => { this.currentView = "chat"; }}>${this.iBack()} Back</button>
          <div class="topbar-divider"></div>
          <span class="topbar-title">Train model</span>
        </div>

        <div class="page-hero">
          <div class="page-hero-label">Knowledge base</div>
          <h1 class="page-hero-title">Train the model</h1>
          <p class="page-hero-sub">The more context you provide, the more accurate the SQL generation becomes. Work through the steps below to build up your knowledge base.</p>
        </div>

        <div class="page-content">
          <div class="train-intro">
            Training adds knowledge to the vector store so the AI generates <strong>accurate SQL for your specific database</strong>.
            Start by importing your schema, then add business rules and worked examples.
          </div>

          <!-- Step 1 -->
          <div class="train-section">
            <div class="train-section-header">
              <div class="train-section-num">1</div>
              <div class="train-section-info">
                <p class="train-section-title">Database schema</p>
                <p class="train-section-desc">Your schema is imported automatically when you connect a database in Settings. Use the button below only to <strong>re-sync after your database structure changes</strong> (new tables or columns).</p>
              </div>
            </div>
            <div class="train-section-body">
              <div class="schema-action-box">
                <div class="schema-action-icon">${this.iDB()}</div>
                <div class="schema-action-info">
                  <strong>Re-import schema</strong>
                  <span>Re-scans all tables and refreshes their definitions in the knowledge base</span>
                </div>
                <button class="btn btn-primary" @click=${this.learnSchema} ?disabled=${this.schemaLoading}>
                  ${this.schemaLoading ? html`${this.iSpin()} Re-importing…` : html`${this.iDB()} Re-import schema`}
                </button>
              </div>
              ${this.schemaToast ? html`<div class="toast-msg ok">${this.iCheck()} ${this.schemaToast}</div>` : nothing}
            </div>
          </div>

          <!-- Step 2 -->
          <div class="train-section">
            <div class="train-section-header">
              <div class="train-section-num">2</div>
              <div class="train-section-info">
                <p class="train-section-title">Add business documentation</p>
                <p class="train-section-desc">Describe metrics, definitions, or rules in plain English — how revenue is calculated, what status codes mean, which columns to use for dates, etc.</p>
              </div>
            </div>
            <div class="train-section-body">
              <div class="field">
                <label class="field-label">${this.iDoc()} Documentation</label>
                <textarea
                  placeholder="Example: Revenue = quantity × unit_price, excluding orders with status = 'refunded'. The fiscal year runs April 1 – March 31."
                  .value=${this.trainDoc}
                  @input=${(e: Event) => { this.trainDoc = (e.target as HTMLTextAreaElement).value; }}
                ></textarea>
              </div>
              <div class="form-actions">
                <button class="btn btn-primary" @click=${this.addDoc}>Add documentation</button>
                ${this.toast(this.docToast, false)}
              </div>
            </div>
          </div>

          <!-- Step 3 -->
          <div class="train-section">
            <div class="train-section-header">
              <div class="train-section-num">3</div>
              <div class="train-section-info">
                <p class="train-section-title">Add table definition (DDL)</p>
                <p class="train-section-desc">Paste a CREATE TABLE statement for tables not yet in the database, or to provide extra context about column meanings.</p>
              </div>
            </div>
            <div class="train-section-body">
              <div class="field">
                <label class="field-label">${this.iCode()} CREATE TABLE statement</label>
                <textarea
                  style="font-family:var(--opendbpylot-font-family-mono);font-size:12.5px;min-height:130px;"
                  placeholder="CREATE TABLE orders (
  id          INTEGER PRIMARY KEY,
  customer_id INTEGER NOT NULL,
  status      TEXT    CHECK(status IN ('pending','shipped','refunded')),
  total       REAL    NOT NULL,
  created_at  TEXT    NOT NULL   -- ISO 8601 timestamp
);"
                  .value=${this.trainDdl}
                  @input=${(e: Event) => { this.trainDdl = (e.target as HTMLTextAreaElement).value; }}
                ></textarea>
              </div>
              <div class="form-actions">
                <button class="btn btn-primary" @click=${this.addDdl}>Add table definition</button>
                ${this.toast(this.ddlToast, false)}
              </div>
            </div>
          </div>

          <!-- Step 4 -->
          <div class="train-section">
            <div class="train-section-header">
              <div class="train-section-num">4</div>
              <div class="train-section-info">
                <p class="train-section-title">Add a question → SQL example</p>
                <p class="train-section-desc">Give the AI a worked example: a natural-language question paired with the SQL that correctly answers it. The more examples you add, the more accurate the AI becomes.</p>
              </div>
            </div>
            <div class="train-section-body">
              <div class="field">
                <label class="field-label">${this.iMsg()} Question (plain English)</label>
                <input type="text"
                  placeholder="What is the total revenue per country for last month?"
                  .value=${this.trainQ}
                  @input=${(e: Event) => { this.trainQ = (e.target as HTMLInputElement).value; }} />
              </div>
              <div class="field">
                <label class="field-label">${this.iCode()} Correct SQL</label>
                <textarea
                  style="font-family:var(--opendbpylot-font-family-mono);font-size:12.5px;min-height:130px;"
                  placeholder="SELECT country, SUM(total) AS revenue
FROM orders
WHERE created_at >= date('now','start of month','-1 month')
  AND created_at <  date('now','start of month')
GROUP BY country
ORDER BY revenue DESC;"
                  .value=${this.trainSql}
                  @input=${(e: Event) => { this.trainSql = (e.target as HTMLTextAreaElement).value; }}
                ></textarea>
              </div>
              <div class="form-actions">
                <button class="btn btn-primary" @click=${this.addSql}>Add example</button>
                ${this.toast(this.sqlToast, false)}
              </div>
            </div>
          </div>
        </div>
      </div>
    `;
  }

  render() {
    const ready = this.settings.ready;
    return html`
      <div class="mobile-overlay ${this.sidebarOpen ? "active" : ""}"
        @click=${() => { this.sidebarOpen = false; }}></div>

      <aside class="sidebar ${this.sidebarOpen ? "open" : ""}">
        <div class="sidebar-header">
          <div class="logo-mark">
            <svg viewBox="0 0 24 24"><path d="M12 2C6.48 2 2 6.48 2 12s4.48 10 10 10 10-4.48 10-10S17.52 2 12 2zm-1 14H9V8h2v8zm4 0h-2V8h2v8z"/></svg>
          </div>
          <span class="logo-text">opendbpylot</span>
          <span class="logo-badge">AI</span>
        </div>

        <div class="sidebar-actions">
          <button class="btn-new-chat" @click=${() => { this.newChat(); this.sidebarOpen = false; }}>
            ${this.iPlus()} New conversation
          </button>
        </div>

        <div class="section-label">History</div>
        <div class="conv-list">
          ${this.conversations.length
            ? this.conversations.map(c => html`
                <div class="conv-item ${c.id === this.activeConv ? "active" : ""}"
                  title="${c.title || "Untitled"}"
                  @click=${() => this.openConversation(c.id)}>
                  <span class="conv-title">${c.title || "Untitled"}</span>
                  <button class="conv-del" title="Delete conversation"
                    @click=${(e: Event) => this.deleteConversation(c.id, e)}>${this.iTrash()}</button>
                </div>`)
            : html`<div class="empty-conv">No conversations yet</div>`}
        </div>

        <div class="sidebar-footer">
          <button class="nav-item ${this.currentView === "train" ? "active" : ""}"
            @click=${() => { this.currentView = "train"; this.sidebarOpen = false; }}>
            ${this.iBook()} Train model
          </button>
          <button class="nav-item ${this.currentView === "settings" ? "active" : ""}"
            @click=${() => { this.currentView = "settings"; this.sidebarOpen = false; }}>
            ${this.iGear()} Settings
            <span class="connection-badge ${ready ? "ok" : "bad"}">${ready ? "Connected" : "Not set"}</span>
          </button>
        </div>
      </aside>

      <main class="main">
        <div class="mobile-header">
          <button class="btn-menu" @click=${() => { this.sidebarOpen = !this.sidebarOpen; }}>
            ${this.iMenu()} Menu
          </button>
          <span style="font-size:14px;font-weight:700;">opendbpylot</span>
        </div>

        ${this.currentView === "chat"
          ? html`<opendbpylot-chat theme="dark" sse-endpoint="/api/opendbpylot/v2/chat_sse"></opendbpylot-chat>`
          : nothing}
        ${this.currentView === "settings" ? this.renderSettings() : nothing}
        ${this.currentView === "train" ? this.renderTrain() : nothing}
      </main>
    `;
  }
}
