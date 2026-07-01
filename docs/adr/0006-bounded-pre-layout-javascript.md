# ADR 0006: Bounded Pre-Layout JavaScript

## Status

Accepted (2026-07-01). **First pass implemented**: `BoaScriptEngine` (Boa, behind
the optional `js` cargo feature) runs inline scripts against a minimal `document`
DOM API and mutates the DOM before styling/layout, opt-in via
`Engine::render_html_with_scripts` and the CLI `--js` flag. Default builds contain
no JS engine and are byte-identical. Builds on ADR 0002 (DOM-based pipeline). The
trait/types and engine live in `crates/htmltopdf/src/script.rs`.

Implemented so far: `document.getElementById`, element `textContent` (get/set),
`getAttribute`/`setAttribute`, `console.log`, and a loop-iteration limit
(`ScriptLimits.max_ticks`). Still to do (see risks/open questions):
`innerHTML`/`createElement`, DOM traversal, heap/wall-time enforcement, and the
Boa-vs-QuickJS default choice.

## Context

The product goal is "full CSS **and JS**" at a fraction of Chromium's memory,
with embarrassingly parallel renders. Everything before this milestone renders
HTML statically: scripts in the document are ignored. Many real documents
(dashboards, invoices, reports) ship templating/personalization logic in
JavaScript that runs once at load and mutates the DOM, after which the page is
static. Supporting that is high value.

Supporting *general* browser JavaScript (event loops, timers, animation,
network, full Web APIs) is explicitly **not** the goal: it would reintroduce the
cost and unpredictability we are differentiating away from. We want a **bounded,
deterministic, pre-layout** execution model.

## Decision

### Pipeline placement

The script stage runs **after the DOM is built and before the style cascade and
box generation**:

```text
HTML -> html5ever -> arena DOM -> [ SCRIPT STAGE ] -> cascade -> box tree -> layout -> PDF
```

Scripts see a complete DOM and mutate it; styling and layout then operate on the
mutated DOM. This models "parse, run document scripts, snapshot, render" — not a
live event loop. There is no post-layout scripting, no reflow-during-script, and
no animation frame loop.

### The seam (already scaffolded)

A trait abstracts the engine so the rest of the pipeline never depends on a
specific runtime:

```rust
pub trait ScriptEngine {
    fn run(&self, dom: &mut Dom, limits: &ScriptLimits) -> ScriptReport;
}
```

- `NoopScriptEngine` (the default, today) leaves the DOM untouched — current
  behavior.
- The real engine plugs in behind a cargo feature, so default builds stay
  dependency-light and fast.
- `inline_scripts(dom)` already collects executable inline `<script>` source in
  document order (skipping `src=` and non-JS `type=`s).

`RenderOptions` will gain an optional script engine + `ScriptLimits`; with none
set, rendering is static exactly as now.

### Engine choice

Behind the trait, two candidates:

- **Boa** (pure Rust): keeps the no-C-dependency, fast-build, easy-cross-compile
  ethos; smaller spec coverage and slower. Preferred default for the `js`
  feature.
- **QuickJS** (via `rquickjs`): far more complete and faster; adds a C
  dependency and build complexity. An opt-in alternative feature for users who
  need broader language coverage.

The trait means we can start with one and swap or offer both without touching
layout.

### Bounded, deterministic execution

Every run is capped by `ScriptLimits` (all hard limits; first one hit stops
execution and the partial DOM is kept, so a render always produces output):

- `max_wall_millis` — wall-clock ceiling for the whole stage.
- `max_ticks` — a deterministic interrupt/instruction budget (bounds infinite
  loops independently of machine speed).
- `max_new_nodes` — caps DOM blow-ups from `innerHTML`/`createElement`.
- `max_heap_bytes` — script heap ceiling.
- `allow_network` (default **false**) — `fetch`/`XHR` are absent or fail closed;
  a render never makes an implicit network call.
- `allow_timers` (default **false**) — timers are ignored; when enabled,
  callbacks are drained **synchronously** up to the tick budget. There is no real
  event loop.

Determinism rules the engine must enforce: no ambient wall-clock or RNG seeding
that varies per run (`Date.now()`/`Math.random()` are fixed or seeded from
explicit input), no filesystem, no threads. Each render gets a **fresh, isolated
realm** — no shared global mutable state — so renders remain independent and
`Send`, preserving the concurrency/RAM properties (ADR 0002).

### DOM API subset (staged)

The engine binds a minimal, growing DOM surface to the arena DOM:

1. Read: `document.getElementById/querySelector(All)`, `element.textContent`,
   attributes, `tagName`, traversal.
2. Mutate: `textContent`, `setAttribute`/`className`/`style`, `createElement`,
   `appendChild`/`removeChild`/`replaceChild`, `innerHTML` (parsed via the same
   html5ever path, counted against `max_new_nodes`).
3. Console (`console.log` → captured into `ScriptReport`, not stdout).

Out of scope initially: layout-reading APIs (`getBoundingClientRect`,
`offsetWidth`) — there is no layout yet when scripts run — plus events, timers
(beyond the bounded drain), network, storage, workers.

## Consequences

- A clean boundary exists now (`script.rs`) with zero behavior change and no new
  dependencies; the default path is byte-identical.
- When a real engine lands, it is feature-gated and isolated, so it cannot
  regress the static-render path or the concurrency model.
- We can support the common "run-once templating" case without taking on a
  browser's cost or unpredictability.

## Risks / open questions

- Binding the arena DOM (index-based, no `Rc`) to a JS engine's object model
  needs a handle/proxy layer; lifetime and mutation-during-iteration semantics
  need care.
- `innerHTML` re-entrancy into html5ever must reuse the custom `TreeSink` and
  respect `max_new_nodes`.
- Whether to expose any layout metrics (a second, post-first-layout script pass)
  is deferred — it would complicate the single-pass model.
- Choosing Boa vs QuickJS as the default `js` feature depends on measured spec
  coverage against real fixtures.
