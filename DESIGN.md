---
name: UCode Portable
description: A precise, portable, evidence-first coding Agent workbench.
colors:
  canvas-deep: "#111312"
  workspace: "#171918"
  panel: "#141615"
  surface: "#202320"
  surface-hover: "#292D2A"
  border: "#2A2D2B"
  text-primary: "#E8E9E7"
  text-muted: "#8B918E"
  accent: "#A8E986"
  accent-ink: "#172114"
  warning: "#E4B86A"
  danger: "#E7867F"
typography:
  title:
    fontFamily: "Segoe UI Variable, Microsoft YaHei UI, system-ui, sans-serif"
    fontSize: "15px"
    fontWeight: 650
    lineHeight: 1.3
    letterSpacing: "-0.01em"
  body:
    fontFamily: "Segoe UI Variable, Microsoft YaHei UI, system-ui, sans-serif"
    fontSize: "14px"
    fontWeight: 400
    lineHeight: 1.65
  label:
    fontFamily: "Segoe UI Variable, Microsoft YaHei UI, system-ui, sans-serif"
    fontSize: "11px"
    fontWeight: 600
    lineHeight: 1.25
  mono:
    fontFamily: "Cascadia Code, SFMono-Regular, Consolas, monospace"
    fontSize: "12px"
    fontWeight: 400
    lineHeight: 1.55
rounded:
  control: "7px"
  compact: "9px"
  surface: "12px"
  composer: "16px"
spacing:
  xs: "4px"
  sm: "8px"
  md: "12px"
  lg: "16px"
  xl: "24px"
components:
  button-primary:
    backgroundColor: "{colors.accent}"
    textColor: "{colors.accent-ink}"
    typography: "{typography.label}"
    rounded: "{rounded.control}"
    padding: "8px 12px"
  button-quiet:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.text-primary}"
    typography: "{typography.label}"
    rounded: "{rounded.control}"
    padding: "7px 10px"
  input:
    backgroundColor: "{colors.canvas-deep}"
    textColor: "{colors.text-primary}"
    typography: "{typography.body}"
    rounded: "{rounded.control}"
    padding: "8px 10px"
---

# Design System: UCode Portable

## Overview

**Creative North Star: "The Field Engineer's Notebook"**

UCode should feel like a well-made instrument carried between machines: quiet at rest, exact when active, and marked by evidence rather than decoration. The conversation is the notebook page; files, Diff, terminal, Git, and Agents unfold as adjacent tools only when the task needs them.

The system rejects generic AI dashboards, glowing glass, purple gradients, full IDE chrome, oversized chat cards, and decorative thinking theatre. Density is earned by the task. Motion explains spatial change and never delays daily work.

**Key Characteristics:**

- Restrained deep forest neutrals with one living green accent.
- Compact desktop density with a readable 65–75 character prose measure.
- Tonal layers and one-pixel boundaries instead of nested cards and heavy shadows.
- Familiar Windows product affordances with precise Agent-specific review states.
- 150–240 ms state motion using one exponential easing curve.

## Colors

The palette is a low-chroma, green-tinted near-black field with a single optimistic signal color.

### Primary

- **Instrument Green:** Reserved for primary actions, active state, successful readiness, and keyboard focus. It must stay below ten percent of the visible surface.

### Secondary

- **Review Amber:** Used only for approval and waiting states.
- **Fault Coral:** Used only for failure, destructive intent, and invalid state.

### Neutral

- **Canvas Deep:** The outermost background and terminal-grade surfaces.
- **Workspace Charcoal:** The conversation reading surface.
- **Panel Graphite:** Navigation and review panels.
- **Quiet Silver:** Secondary text, timestamps, metadata, and inactive icons.

**The One Signal Rule.** One screen has one dominant accent. Green indicates action or readiness; amber and coral never become decoration.

## Typography

**Display Font:** Segoe UI Variable (with Microsoft YaHei UI and system UI fallback)
**Body Font:** Segoe UI Variable (with Microsoft YaHei UI and system UI fallback)
**Label/Mono Font:** Cascadia Code (with SFMono-Regular and Consolas fallback)

**Character:** Native Windows clarity for navigation and prose, with monospace reserved for code, paths, commands, and evidence.

### Hierarchy

- **Title** (650, 15px, 1.3): Conversation titles and persistent panel headings.
- **Body** (400, 14px, 1.65): Assistant prose, limited to 65–75 characters per line.
- **Label** (600, 11px, 1.25): Buttons, tabs, metadata, and compact state labels.
- **Mono** (400, 12px, 1.55): Code, Diff, terminal, Git, paths, and hashes.

**The Evidence Mono Rule.** Monospace communicates machine-verifiable material. It is forbidden for ordinary navigation or prose.

## Elevation

UCode is flat by default. Depth comes from tonal changes, one-pixel boundaries, and spatial movement. Shadows are reserved for overlays, menus, the floating composer, and a panel actively dragged above the workspace.

### Shadow Vocabulary

- **Composer Lift** (`0 12px 36px rgba(0,0,0,.25)`): Anchors the active input above the reading surface.
- **Overlay Lift** (`0 16px 40px rgba(0,0,0,.36)`): Menus, drawers, and approval overlays only.
- **Focus Halo** (`0 0 0 3px rgba(168,233,134,.08)`): Active input and keyboard focus reinforcement.

**The Flat at Rest Rule.** If every surface has a shadow, the hierarchy has failed. Static panels use tone and a one-pixel boundary.

## Components

### Buttons

- **Shape:** Compact, gently curved controls (7px radius).
- **Primary:** Instrument Green with dark ink; one primary action per local surface.
- **Hover / Focus:** Small tonal lift, visible focus outline, no bounce or glow.
- **Secondary / Ghost:** Tonal or transparent, never equal in emphasis to the primary action.

### Chips

- **Style:** Compact one-line metadata with a subtle border and source badge.
- **State:** Project context uses green tint; external read-only context uses amber tint; neither becomes a large card.

### Cards / Containers

- **Corner Style:** 9–12px where grouping is necessary.
- **Background:** One tonal step from the parent surface.
- **Shadow Strategy:** No shadow at rest.
- **Border:** One-pixel low-contrast boundary.
- **Internal Padding:** 8–14px, selected by density rather than a universal value.

### Inputs / Fields

- **Style:** Dark tonal field, one-pixel boundary, 7–16px radius according to role.
- **Focus:** Accent border plus restrained focus halo.
- **Error / Disabled:** Text, icon, and color together communicate state.

### Navigation

- Tabs and rows use familiar rectangular hit targets, quiet default color, tonal hover, and one clear selected state.
- Narrow windows use drawers; desktop widths use resizable persistent panes.
- Dividers expose pointer resize, keyboard resize, double-click reset, and a visible hover target.

### Conversation Turn

- A turn is one user request, optional factual activity, one final answer, and optional approval.
- Assistant prose is not a bubble. User input is a compact anchored surface.
- Transport events and token deltas never appear as log rows.

## Do's and Don'ts

### Do:

- **Do** keep the conversation as the largest and calmest surface.
- **Do** use Instrument Green only for the current action, selection, focus, or ready state.
- **Do** collapse completed execution detail and reveal it progressively.
- **Do** preserve native Windows resizing, DPI behavior, system controls, and keyboard conventions.
- **Do** use one scrollbar vocabulary across every owned scroll surface.
- **Do** show actual files, Diff, commands, tests, and persisted state as evidence.

### Don't:

- **Don't** build generic AI dashboards with purple gradients, glowing glass cards, or decorative “thinking” theatrics.
- **Don't** turn every message, tool result, and status into a large nested card.
- **Don't** imitate full IDE chrome or add dense toolbars that compete with the task.
- **Don't** show raw transport logs, tool JSON, token events, or private reasoning as activity.
- **Don't** use canned capability text or success language without real evidence.
- **Don't** use fixed panes that can hide the composer when the window is resized.
- **Don't** use a colored side stripe wider than one pixel as an accent.
- **Don't** animate layout properties during pane resize; update the split immediately and animate only settle/reset states.
