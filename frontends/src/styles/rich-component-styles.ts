// Plain CSS (as a string) injected into the messages container so the
// dynamically-rendered rich components are styled. Uses the design tokens
// inherited from the host element.
export const richStyles = `
  .rc { margin: 10px 0; }
  .rc-label {
    font-size: 11px; text-transform: uppercase; letter-spacing: .09em;
    color: var(--opendbpylot-foreground-dimmest); margin-bottom: 6px;
  }
  .rc-prose { font-size: 14px; line-height: 1.55; color: var(--opendbpylot-foreground-default); }

  .rc-codewrap { position: relative; }
  .rc-code {
    background: var(--opendbpylot-background-lower); border: 1px solid var(--opendbpylot-outline-dimmer);
    border-radius: var(--opendbpylot-border-radius-sm); padding: 13px 14px; margin: 0;
    overflow-x: auto; font-family: var(--opendbpylot-font-family-mono); font-size: 13px; line-height: 1.5;
    color: var(--opendbpylot-foreground-default);
  }
  .rc-copy {
    position: absolute; top: 8px; right: 8px; cursor: pointer;
    font-size: 11px; color: var(--opendbpylot-foreground-dimmest);
    background: var(--opendbpylot-background-highest); border: 1px solid var(--opendbpylot-outline-dimmer);
    border-radius: 6px; padding: 3px 8px;
  }
  .rc-copy:hover { color: var(--opendbpylot-foreground-default); border-color: var(--opendbpylot-accent-primary-default); }

  .rc-tablewrap { overflow-x: auto; border: 1px solid var(--opendbpylot-outline-dimmer); border-radius: var(--opendbpylot-border-radius-sm); }
  .rc-df table { border-collapse: collapse; width: 100%; font-size: 13px; }
  .rc-df th, .rc-df td { padding: 8px 12px; text-align: left; white-space: nowrap; }
  .rc-df thead th { background: var(--opendbpylot-background-highest); color: var(--opendbpylot-foreground-default); border-bottom: 1px solid var(--opendbpylot-outline-dimmer); }
  .rc-df tbody tr:nth-child(even) { background: rgba(127,127,127,.06); }
  .rc-df tbody td { color: var(--opendbpylot-foreground-dimmest); border-bottom: 1px solid var(--opendbpylot-outline-dimmer); }
  .rc-rowcount { color: var(--opendbpylot-accent-positive-default); font-size: 12px; margin-top: 6px; }

  .rc-note { font-size: 13.5px; padding: 10px 12px; border-radius: var(--opendbpylot-border-radius-sm); border: 1px solid var(--opendbpylot-outline-dimmer); }
  .rc-note-error { color: var(--opendbpylot-accent-negative-default); border-color: var(--opendbpylot-accent-negative-default); background: rgba(220,38,38,.08); }
  .rc-note-warning { color: var(--opendbpylot-accent-warning-default); border-color: var(--opendbpylot-accent-warning-default); background: rgba(217,119,6,.08); }
  .rc-note-info, .rc-note-success { color: var(--opendbpylot-foreground-dimmest); }

  .rc-status { color: var(--opendbpylot-foreground-dimmest); font-size: 13px; }
  .rc-chart { border: 1px solid var(--opendbpylot-outline-dimmer); border-radius: var(--opendbpylot-border-radius-sm); padding: 8px; }
  .rc-fallback { color: var(--opendbpylot-foreground-dimmest); font-family: var(--opendbpylot-font-family-mono); font-size: 12px; }

  /* progress bar */
  .rc-progress-label { display: flex; justify-content: space-between; font-size: 12px; color: var(--opendbpylot-foreground-dimmest); margin-bottom: 6px; }
  .rc-progress-track { height: 8px; background: var(--opendbpylot-background-highest); border-radius: 999px; overflow: hidden; }
  .rc-progress-fill { height: 100%; background: linear-gradient(90deg, var(--opendbpylot-accent-primary-default), var(--opendbpylot-teal)); transition: width .3s ease; }

  /* card */
  .rc-card { border: 1px solid var(--opendbpylot-outline-dimmer); border-radius: var(--opendbpylot-border-radius-sm); padding: 12px; background: var(--opendbpylot-background-highest); }
  .rc-card-title { font-weight: 600; margin-bottom: 4px; }
  .rc-card-body { color: var(--opendbpylot-foreground-dimmest); font-size: 14px; }
  .rc-card-item { color: var(--opendbpylot-foreground-dimmest); font-size: 13px; margin-top: 2px; }

  /* buttons */
  .rc-buttons { display: flex; flex-wrap: wrap; gap: 8px; }
  .rc-btn {
    cursor: pointer; border: 1px solid var(--opendbpylot-outline-dimmer); border-radius: 10px;
    background: var(--opendbpylot-background-highest); color: var(--opendbpylot-foreground-default); padding: 8px 12px; font-size: 13px;
  }
  .rc-btn:hover { border-color: var(--opendbpylot-accent-primary-default); color: var(--opendbpylot-foreground-default); }
`;
