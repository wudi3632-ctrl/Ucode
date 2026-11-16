# UCode Desktop Design Contract

## Layout

The application is a viewport-locked three-region shell: navigation, conversation, and contextual inspector. Each region owns its own overflow. The conversation uses three rows: compact header, scrollable timeline, and persistent composer.

At medium widths the inspector is hidden by default through responsive layout. At narrow widths navigation becomes a compact rail. Both secondary panels can be toggled manually from the conversation header.

## Conversation

- Maximum readable line length is approximately 760px.
- User messages are compact raised bubbles aligned right.
- Assistant messages are borderless prose aligned left.
- Reasoning and tool progress use a collapsible vertical activity rail.
- Approval is the only prominent card because it requires a decision.

## Motion

Use 150–250ms ease-out transitions for panel resizing, focus, item entry, and disclosure. Activity indicators may loop softly while work is running. Respect `prefers-reduced-motion`.

Existing thread content never replays its entrance or streaming motion. Only newly created state moves. Menus originate at their trigger, tabs move laterally, and disclosure controls preserve spatial continuity.

## Color and depth

Neutral charcoal surfaces establish hierarchy through small luminance changes. The green accent communicates local readiness and completion. Amber is reserved for approval; red is reserved for errors. Shadows should be broad and low-opacity.

## Interaction honesty

Every visible control has a real action, an accurate disabled state, or a plain-language unavailable explanation. Right-panel tabs expose real project data. Chat lifecycle actions persist through the Rust storage layer. Closing the window moves UCode to the notification area instead of silently ending or becoming unreachable.
