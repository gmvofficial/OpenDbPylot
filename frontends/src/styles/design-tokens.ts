import { css } from 'lit';

// OpenDbPylot 2.0 design tokens - Data-First Agents branding
// Dark theme is the default; light theme is the override.
export const tokens = css`
  :host {
    /* OpenDbPylot 2.0 Brand Colors */
    --opendbpylot-navy: rgb(2, 61, 96);
    --opendbpylot-cream: rgb(231, 225, 207);
    --opendbpylot-teal: rgb(21, 168, 168);
    --opendbpylot-orange: rgb(254, 93, 38);
    --opendbpylot-magenta: rgb(191, 19, 99);

    /* Color Palette - Dark mode (default) */
    --opendbpylot-background-root: rgb(9, 11, 17);
    --opendbpylot-background-default: rgb(15, 18, 25);
    --opendbpylot-background-higher: rgb(24, 29, 39);
    --opendbpylot-background-highest: rgb(31, 39, 51);
    --opendbpylot-background-subtle: rgb(17, 21, 28);
    --opendbpylot-background-lower: rgb(6, 8, 12);

    --opendbpylot-foreground-default: rgb(248, 250, 252);
    --opendbpylot-foreground-dimmer: rgb(203, 213, 225);
    --opendbpylot-foreground-dimmest: rgb(148, 163, 184);

    --opendbpylot-accent-primary-default: rgb(21, 168, 168);
    --opendbpylot-accent-primary-stronger: rgb(21, 168, 168);
    --opendbpylot-accent-primary-strongest: rgb(2, 61, 96);
    --opendbpylot-accent-primary-subtle: rgba(21, 168, 168, 0.15);
    --opendbpylot-accent-primary-hover: rgb(21, 168, 168);

    --opendbpylot-accent-positive-default: rgb(21, 168, 168);
    --opendbpylot-accent-positive-stronger: rgb(21, 168, 168);
    --opendbpylot-accent-positive-subtle: rgba(21, 168, 168, 0.15);

    --opendbpylot-accent-negative-default: rgb(248, 113, 113);
    --opendbpylot-accent-negative-stronger: rgb(239, 68, 68);
    --opendbpylot-accent-negative-subtle: rgba(248, 113, 113, 0.15);

    --opendbpylot-accent-warning-default: rgb(254, 93, 38);
    --opendbpylot-accent-warning-stronger: rgb(254, 93, 38);
    --opendbpylot-accent-warning-subtle: rgba(254, 93, 38, 0.15);

    --opendbpylot-outline-default: rgba(21, 168, 168, 0.3);
    --opendbpylot-outline-dimmer: rgb(31, 41, 55);
    --opendbpylot-outline-dimmest: rgb(17, 24, 39);
    --opendbpylot-outline-hover: rgb(21, 168, 168);

    /* Typography */
    --opendbpylot-font-family-default: "Space Grotesk", ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue", Arial, "Noto Sans", sans-serif;
    --opendbpylot-font-family-serif: "Roboto Slab", ui-serif, Georgia, serif;
    --opendbpylot-font-family-mono: "Space Mono", ui-monospace, SFMono-Regular, "SF Mono", Monaco, Inconsolata, "Roboto Mono", "Ubuntu Mono", monospace;

    /* Spacing scale */
    --opendbpylot-space-0: 0px;
    --opendbpylot-space-1: 4px;
    --opendbpylot-space-2: 8px;
    --opendbpylot-space-3: 12px;
    --opendbpylot-space-4: 16px;
    --opendbpylot-space-5: 20px;
    --opendbpylot-space-6: 24px;
    --opendbpylot-space-7: 28px;
    --opendbpylot-space-8: 32px;
    --opendbpylot-space-10: 40px;
    --opendbpylot-space-12: 48px;
    --opendbpylot-space-16: 64px;

    /* Border radius */
    --opendbpylot-border-radius-sm: 6px;
    --opendbpylot-border-radius-md: 10px;
    --opendbpylot-border-radius-lg: 14px;
    --opendbpylot-border-radius-xl: 20px;
    --opendbpylot-border-radius-2xl: 24px;
    --opendbpylot-border-radius-full: 9999px;

    /* Shadows - dark defaults */
    --opendbpylot-shadow-xs: 0 1px 2px 0 rgba(0, 0, 0, 0.6);
    --opendbpylot-shadow-sm: 0 1px 3px 0 rgba(0, 0, 0, 0.5), 0 1px 2px -1px rgba(0, 0, 0, 0.5);
    --opendbpylot-shadow-md: 0 4px 6px -1px rgba(0, 0, 0, 0.4), 0 2px 4px -2px rgba(0, 0, 0, 0.4);
    --opendbpylot-shadow-lg: 0 10px 15px -3px rgba(0, 0, 0, 0.4), 0 4px 6px -4px rgba(0, 0, 0, 0.4);
    --opendbpylot-shadow-xl: 0 20px 25px -5px rgba(0, 0, 0, 0.3), 0 8px 10px -6px rgba(0, 0, 0, 0.3);
    --opendbpylot-shadow-2xl: 0 25px 50px -12px rgba(0, 0, 0, 0.6);

    /* Animation durations */
    --opendbpylot-duration-75: 75ms;
    --opendbpylot-duration-100: 100ms;
    --opendbpylot-duration-150: 150ms;
    --opendbpylot-duration-200: 200ms;
    --opendbpylot-duration-300: 300ms;
    --opendbpylot-duration-500: 500ms;
    --opendbpylot-duration-700: 700ms;

    /* Z-index scale */
    --opendbpylot-z-dropdown: 1000;
    --opendbpylot-z-sticky: 1020;
    --opendbpylot-z-fixed: 1030;
    --opendbpylot-z-modal: 1040;
    --opendbpylot-z-popover: 1050;
    --opendbpylot-z-tooltip: 1060;

    /* Chat-specific tokens */
    --opendbpylot-chat-bubble-radius: 18px;
    --opendbpylot-chat-bubble-radius-sm: 12px;
    --opendbpylot-chat-spacing: 16px;
    --opendbpylot-chat-avatar-size: 40px;
  }

  /* Light theme override */
  :host([theme="light"]) {
    --opendbpylot-background-root: rgb(255, 255, 255);
    --opendbpylot-background-default: rgb(231, 225, 207);
    --opendbpylot-background-higher: rgb(244, 246, 248);
    --opendbpylot-background-highest: rgb(229, 231, 235);
    --opendbpylot-background-subtle: rgb(248, 250, 252);
    --opendbpylot-background-lower: rgb(239, 242, 245);

    --opendbpylot-foreground-default: rgb(2, 61, 96);
    --opendbpylot-foreground-dimmer: rgb(71, 85, 105);
    --opendbpylot-foreground-dimmest: rgb(100, 116, 139);

    --opendbpylot-accent-primary-default: rgb(21, 168, 168);
    --opendbpylot-accent-primary-stronger: rgb(2, 61, 96);
    --opendbpylot-accent-primary-strongest: rgb(2, 61, 96);
    --opendbpylot-accent-primary-subtle: rgba(21, 168, 168, 0.1);
    --opendbpylot-accent-primary-hover: rgb(21, 168, 168);

    --opendbpylot-accent-positive-default: rgb(21, 168, 168);
    --opendbpylot-accent-positive-stronger: rgb(2, 61, 96);
    --opendbpylot-accent-positive-subtle: rgba(21, 168, 168, 0.1);

    --opendbpylot-accent-negative-default: rgb(239, 68, 68);
    --opendbpylot-accent-negative-stronger: rgb(220, 38, 38);
    --opendbpylot-accent-negative-subtle: rgba(239, 68, 68, 0.1);

    --opendbpylot-accent-warning-default: rgb(254, 93, 38);
    --opendbpylot-accent-warning-stronger: rgb(254, 93, 38);
    --opendbpylot-accent-warning-subtle: rgba(254, 93, 38, 0.1);

    --opendbpylot-outline-default: rgba(21, 168, 168, 0.3);
    --opendbpylot-outline-dimmer: rgb(241, 245, 249);
    --opendbpylot-outline-dimmest: rgb(248, 250, 252);
    --opendbpylot-outline-hover: rgb(21, 168, 168);

    --opendbpylot-shadow-xs: 0 1px 2px 0 rgba(0, 0, 0, 0.05);
    --opendbpylot-shadow-sm: 0 1px 3px 0 rgba(0, 0, 0, 0.1), 0 1px 2px -1px rgba(0, 0, 0, 0.1);
    --opendbpylot-shadow-md: 0 4px 6px -1px rgba(0, 0, 0, 0.1), 0 2px 4px -2px rgba(0, 0, 0, 0.1);
    --opendbpylot-shadow-lg: 0 10px 15px -3px rgba(0, 0, 0, 0.1), 0 4px 6px -4px rgba(0, 0, 0, 0.1);
    --opendbpylot-shadow-xl: 0 20px 25px -5px rgba(0, 0, 0, 0.1), 0 8px 10px -6px rgba(0, 0, 0, 0.1);
    --opendbpylot-shadow-2xl: 0 25px 50px -12px rgba(0, 0, 0, 0.25);
  }
`;
