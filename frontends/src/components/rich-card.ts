import { LitElement, html, css } from 'lit';
import { customElement, property } from 'lit/decorators.js';
import { tokens } from '../styles/design-tokens.js';

export interface CardAction {
  label: string;
  action: string;
  variant?: 'primary' | 'secondary';
}

@customElement('rich-card')
export class RichCard extends LitElement {
  static styles = [
    tokens,
    css`
      :host {
        display: block;
        margin-bottom: var(--opendbpylot-space-4);
        font-family: var(--opendbpylot-font-family-default);
      }

      .card {
        border: 1px solid var(--opendbpylot-outline-default);
        border-radius: var(--opendbpylot-border-radius-lg);
        background: var(--opendbpylot-background-default);
        box-shadow: var(--opendbpylot-shadow-sm);
        overflow: hidden;
        transition: box-shadow var(--opendbpylot-duration-200) ease;
      }

      .card:hover {
        box-shadow: var(--opendbpylot-shadow-md);
      }

      .card-header {
        display: flex;
        align-items: center;
        padding: var(--opendbpylot-space-4) var(--opendbpylot-space-5);
        background: var(--opendbpylot-background-higher);
        border-bottom: 1px solid var(--opendbpylot-outline-default);
        gap: var(--opendbpylot-space-3);
      }

      .card-header.collapsible {
        cursor: pointer;
      }

      .card-icon {
        font-size: 1.25rem;
        display: flex;
        align-items: center;
      }

      .card-title-section {
        flex: 1;
      }

      .card-title {
        margin: 0;
        font-size: 1rem;
        font-weight: 600;
        color: var(--opendbpylot-foreground-default);
      }

      .card-subtitle {
        margin: var(--opendbpylot-space-1) 0 0 0;
        font-size: 0.875rem;
        color: var(--opendbpylot-foreground-dimmer);
      }

      .card-status {
        padding: var(--opendbpylot-space-1) var(--opendbpylot-space-2);
        border-radius: var(--opendbpylot-border-radius-md);
        font-size: 0.75rem;
        font-weight: 600;
        text-transform: uppercase;
      }

      .card-status.status-success {
        background: #d4edda;
        color: #155724;
      }

      .card-status.status-warning {
        background: #fff3cd;
        color: #856404;
      }

      .card-status.status-error {
        background: #f8d7da;
        color: #721c24;
      }

      .card-status.status-info {
        background: #d1ecf1;
        color: #0c5460;
      }

      .card-toggle {
        background: none;
        border: none;
        cursor: pointer;
        font-size: 1rem;
        color: var(--opendbpylot-foreground-dimmer);
        padding: var(--opendbpylot-space-1);
        border-radius: var(--opendbpylot-border-radius-sm);
        transition: background-color var(--opendbpylot-duration-200) ease;
      }

      .card-toggle:hover {
        background: var(--opendbpylot-background-root);
      }

      .card-content {
        padding: var(--opendbpylot-space-4) var(--opendbpylot-space-5);
        line-height: 1.5;
        color: var(--opendbpylot-foreground-default);
        transition: all var(--opendbpylot-duration-200) ease;
        overflow: hidden;
      }

      .card-content.collapsed {
        max-height: 0;
        padding-top: 0;
        padding-bottom: 0;
      }

      .card-content h1,
      .card-content h2,
      .card-content h3 {
        margin: var(--opendbpylot-space-2) 0;
        font-weight: 600;
      }

      .card-content h1 {
        font-size: 1.5rem;
      }

      .card-content h2 {
        font-size: 1.25rem;
      }

      .card-content h3 {
        font-size: 1.125rem;
      }

      .card-content p {
        margin: var(--opendbpylot-space-2) 0;
      }

      .card-content ul {
        margin: var(--opendbpylot-space-2) 0;
        padding-left: var(--opendbpylot-space-5);
      }

      .card-content li {
        margin: var(--opendbpylot-space-1) 0;
      }

      .card-content code {
        background: var(--opendbpylot-background-higher);
        padding: var(--opendbpylot-space-1) var(--opendbpylot-space-2);
        border-radius: var(--opendbpylot-border-radius-sm);
        font-family: monospace;
        font-size: 0.875em;
      }

      .card-content strong {
        font-weight: 600;
      }

      .card-actions {
        padding: var(--opendbpylot-space-3) var(--opendbpylot-space-5);
        background: var(--opendbpylot-background-root);
        border-top: 1px solid var(--opendbpylot-outline-default);
        display: flex;
        gap: var(--opendbpylot-space-2);
      }

      .card-action {
        padding: var(--opendbpylot-space-2) var(--opendbpylot-space-4);
        border-radius: var(--opendbpylot-border-radius-md);
        border: 1px solid var(--opendbpylot-outline-default);
        background: var(--opendbpylot-background-default);
        color: var(--opendbpylot-foreground-default);
        cursor: pointer;
        font-size: 0.875rem;
        font-weight: 500;
        transition: all var(--opendbpylot-duration-200) ease;
      }

      .card-action:hover {
        background: var(--opendbpylot-background-higher);
      }

      .card-action.primary {
        background: var(--opendbpylot-accent-primary-default);
        color: white;
        border-color: var(--opendbpylot-accent-primary-default);
      }

      .card-action.primary:hover {
        background: var(--opendbpylot-accent-primary-stronger);
      }
    `
  ];

  @property() title = '';
  @property() subtitle = '';
  @property() content = '';
  @property() icon = '';
  @property() status: 'info' | 'success' | 'warning' | 'error' = 'info';
  @property({ type: Array }) actions: CardAction[] = [];
  @property({ type: Boolean }) collapsible = false;
  @property({ type: Boolean }) collapsed = false;
  @property({ type: Boolean }) markdown = false;
  @property() theme: 'light' | 'dark' = 'dark';

  private _toggleCollapsed() {
    if (this.collapsible) {
      this.collapsed = !this.collapsed;
    }
  }

  private _renderMarkdown(text: string): string {
    return text
      .replace(/^### (.*$)/gm, '<h3>$1</h3>')
      .replace(/^## (.*$)/gm, '<h2>$1</h2>')
      .replace(/^# (.*$)/gm, '<h1>$1</h1>')
      .replace(/\*\*(.*?)\*\*/g, '<strong>$1</strong>')
      .replace(/\*(.*?)\*/g, '<em>$1</em>')
      .replace(/`([^`]+)`/g, '<code>$1</code>')
      .replace(/^- (.*$)/gm, '<li>$1</li>')
      .replace(/(<li>.*<\/li>)/s, '<ul>$1</ul>')
      .replace(/\n\n/g, '</p><p>')
      .replace(/^(?!<[h|u|l|p])(.+)$/gm, '<p>$1</p>');
  }

  render() {
    const contentHtml = this.markdown
      ? html`<div class="card-content ${this.collapsed ? 'collapsed' : ''}" .innerHTML=${this._renderMarkdown(this.content)}></div>`
      : html`<div class="card-content ${this.collapsed ? 'collapsed' : ''}">${this.content}</div>`;

    return html`
      <div class="card">
        <div class="card-header ${this.collapsible ? 'collapsible' : ''}"
             @click=${this._toggleCollapsed}>
          ${this.icon ? html`<span class="card-icon">${this.icon}</span>` : ''}
          <div class="card-title-section">
            <h3 class="card-title">${this.title}</h3>
            ${this.subtitle ? html`<p class="card-subtitle">${this.subtitle}</p>` : ''}
          </div>
          ${this.status ? html`<span class="card-status status-${this.status}">${this.status}</span>` : ''}
          ${this.collapsible ? html`
            <button class="card-toggle">${this.collapsed ? '▶' : '▼'}</button>
          ` : ''}
        </div>
        ${contentHtml}
        ${this.actions.length > 0 ? html`
          <div class="card-actions">
            ${this.actions.map(action => html`
              <button class="card-action ${action.variant || 'secondary'}"
                      @click=${() => this._handleAction(action.action)}>
                ${action.label}
              </button>
            `)}
          </div>
        ` : ''}
      </div>
    `;
  }

  private async _handleAction(action: string) {
    this.dispatchEvent(new CustomEvent('card-action', {
      detail: { action },
      bubbles: true,
      composed: true
    }));

    const opendbpylotChat = document.querySelector('opendbpylot-chat') as any;
    if (opendbpylotChat && typeof opendbpylotChat.sendMessage === 'function') {
      try {
        await opendbpylotChat.sendMessage(action);
      } catch (error) {
        console.error('Error sending action:', error);
      }
    }
  }
}

declare global {
  interface HTMLElementTagNameMap {
    'rich-card': RichCard;
  }
}
