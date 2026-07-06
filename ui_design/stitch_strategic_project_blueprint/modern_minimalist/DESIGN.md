---
name: Modern Minimalist
colors:
  surface: '#f7f9fb'
  surface-dim: '#d8dadc'
  surface-bright: '#f7f9fb'
  surface-container-lowest: '#ffffff'
  surface-container-low: '#f2f4f6'
  surface-container: '#eceef0'
  surface-container-high: '#e6e8ea'
  surface-container-highest: '#e0e3e5'
  on-surface: '#191c1e'
  on-surface-variant: '#3b494c'
  inverse-surface: '#2d3133'
  inverse-on-surface: '#eff1f3'
  outline: '#6b7a7d'
  outline-variant: '#bac9cc'
  surface-tint: '#006875'
  primary: '#006875'
  on-primary: '#ffffff'
  primary-container: '#00e5ff'
  on-primary-container: '#00626e'
  inverse-primary: '#00daf3'
  secondary: '#505f76'
  on-secondary: '#ffffff'
  secondary-container: '#d0e1fb'
  on-secondary-container: '#54647a'
  tertiary: '#765a00'
  on-tertiary: '#ffffff'
  tertiary-container: '#fec931'
  on-tertiary-container: '#6f5500'
  error: '#ba1a1a'
  on-error: '#ffffff'
  error-container: '#ffdad6'
  on-error-container: '#93000a'
  primary-fixed: '#9cf0ff'
  primary-fixed-dim: '#00daf3'
  on-primary-fixed: '#001f24'
  on-primary-fixed-variant: '#004f58'
  secondary-fixed: '#d3e4fe'
  secondary-fixed-dim: '#b7c8e1'
  on-secondary-fixed: '#0b1c30'
  on-secondary-fixed-variant: '#38485d'
  tertiary-fixed: '#ffdf96'
  tertiary-fixed-dim: '#f3bf26'
  on-tertiary-fixed: '#251a00'
  on-tertiary-fixed-variant: '#594400'
  background: '#f7f9fb'
  on-background: '#191c1e'
  surface-variant: '#e0e3e5'
typography:
  headline-lg:
    fontFamily: Hanken Grotesk
    fontSize: 30px
    fontWeight: '600'
    lineHeight: 38px
    letterSpacing: -0.02em
  headline-lg-mobile:
    fontFamily: Hanken Grotesk
    fontSize: 24px
    fontWeight: '600'
    lineHeight: 32px
    letterSpacing: -0.01em
  headline-md:
    fontFamily: Hanken Grotesk
    fontSize: 20px
    fontWeight: '600'
    lineHeight: 28px
  body-lg:
    fontFamily: Hanken Grotesk
    fontSize: 16px
    fontWeight: '400'
    lineHeight: 24px
  body-md:
    fontFamily: Hanken Grotesk
    fontSize: 14px
    fontWeight: '400'
    lineHeight: 20px
  label-md:
    fontFamily: Hanken Grotesk
    fontSize: 12px
    fontWeight: '500'
    lineHeight: 16px
    letterSpacing: 0.02em
  label-sm:
    fontFamily: Hanken Grotesk
    fontSize: 11px
    fontWeight: '600'
    lineHeight: 14px
rounded:
  sm: 0.5rem
  DEFAULT: 1rem
  md: 1.5rem
  lg: 2rem
  xl: 3rem
  full: 9999px
spacing:
  unit: 4px
  gutter: 16px
  margin-mobile: 16px
  margin-desktop: 32px
  container-max-width: 1100px
---

## Brand & Style

This design system is built on the principles of utility, clarity, and breathability. It draws heavily from **Minimalism** with a focus on functional transparency, similar to the LocalSend aesthetic. The target audience values efficiency and a distraction-free environment, requiring an interface that feels lightweight and unobtrusive.

The visual narrative is driven by generous white space and the elimination of non-functional decoration. By stripping away heavy shadows and complex gradients, the UI allows content and actions to take center stage. The emotional response is one of calm and reliability, achieved through a "soft-utilitarian" approach where clean lines meet approachable, rounded forms.

## Colors

The palette is anchored in a pristine white base to maximize the sense of "air." 

- **Primary:** The electric cyan is retained as the sole accent color. It is used surgically for primary actions, active states, and critical indicators to ensure it never overwhelms the layout.
- **Secondary:** A muted slate gray is used for secondary icons and supporting text, providing enough contrast for legibility without breaking the minimalist harmony.
- **Neutral:** Very light gray tints are used for surface demarcations, such as subtle borders or background offsets for input fields and cards.
- **Surface:** The background remains pure white (`#FFFFFF`) to ensure a high-quality, "paper-like" digital experience.

## Typography

This design system utilizes **Hanken Grotesk** across all roles to maintain a cohesive, professional, and contemporary look. The typeface's geometric yet approachable structure complements the rounded UI elements.

Variations in font size are intentionally minimized to reduce visual noise. Hierarchy is established through weight shifts (SemiBold for headings, Regular for body) and subtle color changes rather than drastic scale jumps. For mobile, large headlines are scaled down to ensure they do not dominate the viewport, maintaining the system's "quiet" personality.

## Layout & Spacing

The layout follows a **fluid grid** model with a hard-cap on container width to prevent line lengths from becoming unreadable on ultra-wide monitors. 

- **Spacing Rhythm:** Based on a 4px baseline, ensuring all margins and paddings are multiples of 4 or 8.
- **Desktop:** A 12-column grid with 16px gutters and 32px outer margins. Content is often centered in a "slim" container to emphasize focus.
- **Mobile:** A 4-column grid with 16px margins. Elements should typically span the full width of the grid to maximize touch targets.
- **Density:** High whitespace is prioritized. Elements are never crowded; if a screen feels "busy," increase the padding rather than adding dividers.

## Elevation & Depth

This design system avoids traditional drop shadows in favor of **Tonal Layers** and **Low-contrast outlines**.

- **Surfaces:** Depth is communicated through color-stepping. The main background is white; secondary containers or "cards" use a light gray (`#F1F5F9`) background or a 1px border in a slightly darker neutral.
- **Interactions:** Hover states are indicated by a subtle shift in background tone (e.g., White to Light Gray) rather than an increase in shadow.
- **Active State:** Only the active element may use a subtle, highly-diffused tint of the primary color to suggest it is "lifted" or selected.

## Shapes

The shape language is defined by high **roundedness (Pill-shaped)**. This softens the minimalist aesthetic, preventing it from appearing too sterile or "industrial." 

Standard components like buttons and tags use maximum corner radii (pill), while larger containers like cards use a 1rem (16px) radius. This consistency in curvature creates a friendly, approachable interface that feels safe and intuitive to navigate.

## Components

- **Buttons:** Primary buttons are solid Electric Cyan with white text, using a pill shape. Secondary buttons use a light gray fill or a simple 1px outline with Slate text. No shadows or glows are allowed.
- **Inputs:** Fields are defined by a 1px neutral border. Upon focus, the border transitions to the primary color. Backgrounds should be pure white.
- **Cards:** Cards are flat. Use a 1px border (`#E2E8F0`) or a very subtle background fill to separate them from the main surface. Avoid shadows entirely.
- **Chips/Tags:** Used for categorization. These are small, pill-shaped elements with a light tint of the primary color and dark text for maximum legibility.
- **Lists:** Clean rows separated by thin, light-gray lines or simply by whitespace. Icons within lists should be monochromatic Slate.
- **Switches/Checkboxes:** Use the primary color for the "On" state. The "Off" state should be a neutral gray, ensuring the component disappears into the UI when not active.