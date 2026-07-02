# ADR 0009: Live-DOM Tree Mutation (`createElement`/`appendChild`/`removeChild`) — and No Mid-Script Layout Reads

Status: **Accepted** — 2026-07-02

## Context

ADR 0008's spike proved structural pre-layout mutation (`innerHTML =`) works
within the engine's low-RAM / parallel-render constraints. `innerHTML` is a
sledgehammer, though: real templating scripts build documents node by node —
`createElement`, `createTextNode`, `appendChild`, `removeChild`. This ADR covers
adding that surface, and settles the question ADR 0008 deliberately deferred:
**should scripts be able to read layout (`getBoundingClientRect`,
`offsetWidth`, …) mid-run?**

## Decision 1: node-by-node tree mutation — **yes**

The arena model absorbs it naturally:

- `Dom::create_element(tag)` / `create_text_node(text)` push a **detached** node
  (`parent: None`) onto the arena. Never-attached nodes are simply unreachable
  from the root and never rendered — no cleanup pass needed.
- `Dom::append_child(parent, child)` attaches, and — because appending an
  attached node **moves** it — also reparents: remove from old parent's child
  list, push onto the new one. `O(children)` per call, no allocation beyond the
  child-list push. A cycle guard (walk `parent` links from the target; refuse if
  the child is an ancestor) keeps the tree a tree; illegal moves return `false`
  rather than throwing, so a buggy script degrades instead of killing the render.
- `Dom::remove_child(parent, child)` detaches; the subtree stays orphaned in the
  arena. **We deliberately do not free or compact** — a render is short-lived,
  the arena drops wholesale at the end, and orphans cost only their node size.
  A script that churns (create + remove in a loop) is bounded by the same
  `max_new_nodes` budget as everything else, so the arena cannot balloon.

Budget accounting: each `createElement`/`createTextNode` draws one node from
`max_new_nodes` (returning `null` past the cap, same fail-soft pattern as
`innerHTML`). `appendChild` draws nothing — moves don't grow the arena.

Both invariants hold trivially: nodes are plain `Vec` entries (no `Rc`, still
`Send`), and peak RAM grows only by what the budget allows.

## Decision 2: mid-script layout reads — **no (rejected for now)**

**For** (why browsers have it): measurement-driven layout — "shrink this font
until the heading fits", pagination-aware templating, JS chart libraries that
size to their container.

**Against — and decisive here:**

1. **It inverts the pipeline.** Today: scripts → cascade → box tree → layout →
   paint, each stage running exactly once, streaming. A layout read mid-script
   forces style+layout of the *partial* DOM at that instant — layout becomes
   re-entrant and incremental, which is the complexity cliff that makes browsers
   browsers. It would be the single largest architectural change possible, to
   serve scripts we have not yet seen demanded.
2. **It breaks the cost model.** One render = one layout pass is what makes
   per-render RAM/CPU predictable and workers cheap to schedule. A script
   calling `getBoundingClientRect` in a loop buys `O(reads × layout)` work.
3. **There is a cheaper 80% substitute.** Most "measure" use-cases in PDF
   generation are really *font-metric* questions ("how wide is this string at
   12pt?"). If demand materializes, expose a `measureText(text, size)` host
   function backed by the existing shaping/AFM metrics — no layout pass, no
   re-entrancy — before ever considering real layout reads.

Revisit when a concrete user script needs true box geometry; the trigger for
reopening this is demand for element *positions*, not text *widths*.

## Consequences

- Scripts can now build a whole document from an empty `<body>` (there is an
  end-to-end test proving a script-built-only document renders).
- Script-created nodes flow through the normal cascade, so they pick up
  stylesheet rules (classes set via `setAttribute`) exactly like parsed nodes.
- `document.body` is exposed as the natural attachment point.
- Still missing from the DOM surface (deliberately, until demanded):
  `insertBefore`, `cloneNode`, `querySelector(All)`, `parentNode`/`children`
  traversal from JS, events, timers.
