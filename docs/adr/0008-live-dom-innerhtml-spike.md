# ADR 0008: Live-DOM `innerHTML` Spike (Structural Pre-Layout Mutation)

Status: **Accepted (spike)** — 2026-07-02

## Context

ADR 0006 introduced a *bounded pre-layout* JavaScript stage: inline scripts run
once, after the DOM is built and before styling/layout, mutating the DOM in
place. The first cut only supported *value* mutations (`textContent`,
`get/setAttribute`). The open question — flagged as the project's biggest
"unsure if it'll work" feature — was whether **structural** DOM mutation
(`innerHTML`, node creation) could be supported **without breaking the two
properties the whole engine is built on: low RAM per render and embarrassingly-
parallel, `Send` renders.**

This ADR records a time-boxed spike that answered it.

## What was built

- `Dom::set_inner_html(node, html)` — parses the markup into a scratch DOM and
  **grafts** its `<body>` children into the live arena under `node` (cross-arena
  deep copy with id remapping). `Dom::inner_html(node)` serializes children back.
  Bounded by the existing `max_new_nodes` script budget.
- The Boa bindings expose `element.innerHTML` (get/set).

## Findings

The spike **succeeds** for structural pre-layout mutation:

1. **It works.** `innerHTML =` injecting 300 / 3000 paragraphs reflows and
   re-paginates end-to-end (1 → 12 / 82 pages). Reflow + re-pagination come
   **for free**: because scripts run *before* layout, the mutated DOM flows
   through the normal cascade → box tree → layout → pagination pipeline. No new
   layout plumbing was needed.
2. **RAM stays bounded.** Peak RSS: static 3.6 MB; `--js` 12 pages 10.4 MB;
   `--js` 82 JS-generated pages 25.9 MB. The cost is a ~7 MB Boa baseline plus
   content — still "tens of MB", consistent with the thesis.
3. **Concurrency is preserved.** The engine is created per render, isolated, with
   no shared global state; each worker independently pays the Boa baseline. The
   `Send` / linear-core-scaling property is unaffected.

## What this does *not* cover (the genuinely hard part, deferred)

- **Mid-script layout reads** (`getBoundingClientRect`, `offsetHeight`,
  resolved `getComputedStyle`). These require running layout *during* script
  execution and re-running after further mutation — an iterative script↔layout
  loop that may not converge and that complicates the single-pass, `Send` model.
  This is the real research risk and is intentionally **not** attempted here.
- Event loops / timers / `requestAnimationFrame` (time-varying DOM) — largely
  meaningless for a static PDF.

## Decision

Adopt structural pre-layout mutation (`innerHTML`) as a natural extension of the
ADR 0006 bounded stage — it covers the overwhelmingly common "dynamic HTML → PDF"
cases (templating, personalization, data-driven tables/lists) at bounded cost.
Keep the layout-reading / live-reflow-loop capability out of scope until there is
a concrete need and a design that preserves the RAM/concurrency guarantees.

## Consequences

- `innerHTML` is available behind the `js` feature via
  `Engine::render_html_with_scripts` / the CLI `--js` flag; default builds are
  unchanged.
- The `max_new_nodes` budget now also caps `innerHTML` blow-ups.
- Next steps if pursued further: `document.createElement`/`appendChild`,
  `removeChild`, and a decision record for (or against) mid-script layout reads.
