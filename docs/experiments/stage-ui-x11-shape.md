# Stage UI probe: X11 visual vs input shape

Follow-up to [Stage UI probe 2](stage-ui-poc-2.md). Still **not a product
path.** Production `ene-stage` is unchanged.

Code: `crates/ene-stage-poc/`

| Binary | Role |
|---|---|
| `ene-stage-poc-x11-shape` | Experiment D2 cases (`T1`, `T3`, `T4`, `T5`, `T5m`, `T6`, `T7u`, `T7d`, `T7o`, `input`, `both`) |
| `ene-stage-poc-click-sink` | Separate process. Logs `SINK PRESS` / `SINK RELEASE` / `SINK CLICK` with `x`/`y` |

This report is **xfwm4 on this Cloud Agent VM**. Other WMs were not
installed. Do not read it as "X11 in general."

```sh
DISPLAY=:1 WGPU_BACKEND=vulkan env -u WAYLAND_DISPLAY \
  cargo build --release -p ene-stage-poc --bins
ENE_STAGE_POC_SECONDS=8 ./target/release/ene-stage-poc-x11-shape T1
ENE_STAGE_POC_SECONDS=8 ./target/release/ene-stage-poc-x11-shape T5
```

Click-through success is always both: the overlay does **not** get the
event, **and** `ene-stage-poc-click-sink` does.

---

## Environment

| Item | Value |
|---|---|
| OS | Linux 6.12, Xorg 21.1.11 |
| Desktop | XFCE |
| X11 WM | **xfwm4** (compositing on) |
| Other WMs | not present (no Mutter / KWin / Openbox / i3 / bspwm) |
| Display | `DISPLAY=:1` (`WAYLAND_DISPLAY` unset for these runs) |
| GPU | no `/dev/dri`, llvmpipe / Mesa 25.2.8, Vulkan |
| Overlay | 800×600 at +80+80, transparent, always-on-top |
| Sink | 1000×700 at +0+0, separate process |
| VRM | `ene_vrm::minimal` fixture |

SHAPE 1.1, XFixes 5.0. SET uses `XShapeCombineRectangles`, not an XFixes
region. `ShapeClip` on the client stayed `800×600+0+0` in every case.
Visible clipping follows **Bounding**, not Clip.

Every dump prints:

```
client=0x…
frame=0x… | none
Bounding(client): …
Input(client): …
Clip(client): …
Bounding(frame): …
Input(frame): …
Clip(frame): …
effective_input_shape=…
```

D2 always SET both client and frame when a frame exists (unlike
Experiment D's default `ENE_STAGE_POC_X11_TARGET=client`).

---

## Question

Can X11 keep

visual geometry ≠ interaction geometry

while another process still receives click-through on glow / shadow /
particles / transparent margin?

Ideal:

- Bounding = VRM visual ∪ bubble ∪ shadow ∪ glow ∪ particles
- Input = VRM coarse colliders ∪ interactive UI

---

## Test 1: Bounding > Input (single rects)

Window 800×600. Bounding `600×400+100+100`. Input `200×100+300+250`.
Cyan field in Bounding. Dark "CLICKABLE" bubble in Input. No VRM.

| Probe | Overlay | Sink |
|---|---|---|
| Input centre (screen 480,380 → overlay 400,300) | `OVERLAY PRESS … layer=Interaction` | no new click |
| Bounding-only (screen 230,230) | no overlay event | `SINK PRESS` (campaign run) |
| Outside Bounding (screen 100,100) | no overlay event | `SINK PRESS x=99 y=43` |

`shape::get_rectangles` at t=50 ms, 200 ms, 1000 ms, and finish:

- Bounding(client)=`600×400+100+100`
- Input(client)=`200×100+300+250`
- `effective_input_shape=MatchesRequested`
- `wm_resets=0`

xfwm4 did **not** restore Input to the full window when Bounding was a
single larger rect and Input was a single smaller rect.

---

## Test 2: effective Input after xfwm4 rewrite

Do not guess from code. Measured:

| Setup | Input after ~200 ms (`get_rectangles`) | Click-effective region |
|---|---|---|
| T1 single rects | requested Input (MatchesRequested) | requested Input |
| Multi-rect Input (T3/T5/T7) | YX-banded rewrite of the union, **not** full window | that banded list (clicks match it, not 800×600) |
| Bounding = Input multi-rect (`both`, T4) | same banded list on Bounding and Input (`MatchesBounding`) | that silhouette |
| Complex Bounding ≠ Input (T3/T5) | Input banded as above. Bounding becomes **`800×600+0+0`** | Input banded list. Bounding no longer clips |

Candidates from the brief:

- A full original window: Input is **not** this when Bounding+Input are set, except T3/T5 Bounding which *is* full window
- B current Bounding: true for `both` / T4 (`MatchesBounding`)
- C client rect / D frame rect: not what Input became
- E other: multi-rect Input is YX-banded (xfwm4 rewrites overlapping rects). Bubble `277×139+40+79` became `277×48+40+79` plus extra strips

Click tests, not the dump string, decide passthrough. After rewrite,
glow / particle / transparent still reached the sink. Interactive
bubble / VRM still reached the overlay.

---

## Test 3 / 5: visual-only Slint + VRM

Bubble + glow + shadow + yellow particle + `ene_vrm::minimal`.
Bounding SET to the visual union (many rects). Input SET to bubble ∪
VRM coarse parts.

T5 replay (`env -u WAYLAND_DISPLAY`):

| Click | Overlay | Sink |
|---|---|---|
| Glow (screen 100,200) | none | `SINK PRESS x=99 y=143` |
| Bubble (screen 200,200) | `OVERLAY PRESS x=120 y=120 layer=Interaction` | none |
| VRM (screen 480,450) | `OVERLAY PRESS x=400 y=370 layer=Interaction` | none |
| Particle (screen 140,600) | none | `SINK PRESS x=139 y=543` |

So the three classes (interactive / visual-only / background) are
distinguishable by real cross-process clicks, even after xfwm4
rebinds Bounding to the full client.

The WM does **not** keep Bounding = the visual union. It keeps Input as
a banded form of the interaction union. Visuals still draw because
Bounding became the full window.

---

## Test 4: Bounding clipping

Bounding SET equal to interaction (glow / particle / "hair" overflow).
After rewrite, Bounding = Input = the banded interaction silhouette.
`Clip` stayed full window.

The yellow particle at `720,20` is inside the GPU window and outside
that silhouette. It is **not** visible in the T4 screenshot. T3/T5
screenshots still show it. GPU-drawn pixels outside Bounding are
clipped by X11 Bounding.

**Visual footprint on screen = Bounding.** If product code needs a
visible glow, that glow's AABB must be inside Bounding. A full-window
Bounding (T3/T5 after WM rewrite) shows everything the GPU drew,
including transparent margin.

---

## Test 6: ShapeNotify Input reapply

T6 SET the same geometry as T5, then on every Input mismatch called
`XShapeCombineRectangles` again. 26.8 s, mixed clicks on bubble / glow
/ background.

| Metric | Value |
|---|---|
| `REAPPLY_INPUT` | **4052** |
| reapply rate | **~151 /s** |
| `ShapeNotify` Input | continuous, ~6 ms apart |
| SET time | 130–600 µs each |
| `wm_reapply_fight` | **true** |

xfwm4 does not restore Input to 800×600 here. It YX-bands the rect
list. `rects_match` against the client's requested list fails every
time, so the client SET, the WM bands, ShapeNotify, repeat.

That is a fight. Not a product candidate. "It flickered through" is not
adoption.

Without reapply, `wm_resets` is **1** (one rewrite after the first
SET). Input then stays at the banded list.

---

## Test 7: undecorated / override-redirect / decorated

| Mode | Frame | Bounding after 1 s | Input after 1 s | Click-through |
|---|---|---|---|---|
| Undecorated managed (T7u, Stage-like) | yes | full window (complex union) | YX-banded interaction | glow → sink, bubble → overlay |
| override_redirect (T7o) | none (`parent=root`) | full window | same banded Input | bubble → overlay |
| Decorated (T7d) | yes, frame Bounding is the decorated silhouette | client Bounding full | frame Input includes title-bar strips | title bar offsets hits. Not the Stage window |

Undecorated + always-on-top is the Stage case. override_redirect is
not required for the split. Decorated is a bad default.

Compositor transparency (ARGB) worked in all three.

---

## Test 8: other WMs

Not run. Only xfwm4 is on this VM. No extra WM was installed.

---

## Dynamic movement (T5m)

After keeping dirty flags until SET actually applies (not per-frame
overwrite):

| `ENE_STAGE_POC_REGION_MS` | bounding_hz | input_hz | combined SET µs | CPU user / 10 s |
|---|---|---|---|---|
| 30 | 14.6 | 8.4 | 795 | 25450 ms |
| 50 | 14.6 | 8.5 | 712 | 25640 ms |
| 100 | 9.7 | 7.1 | 588 | 25490 ms |

30 ms and 50 ms match because the 2 px dirty threshold, not the rate
limit, is the bottleneck. 100 ms binds. Combined SET is under 1 ms.
~8–15 updates/s is enough, same conclusion as Experiment D.

CPU is lavapipe redraw, not SHAPE.

Stale hits: T5m glow still reached the sink, bubble/VRM still reached
the overlay while the bubble Y and VRM X were waving. No leftover
click-through hole showed up in those clicks. Bounding for T5m was
still rewritten to full window, so visual clipping was not the
limiting factor.

---

## Results table

Values are from this xfwm4 session plus the click-sink process.

| Configuration | Visual-only visible | UI clickable | VRM clickable | Visual-only click-through | Background click-through | Stable under xfwm4 |
|---|---|---|---|---|---|---|
| Input only | yes (Bounding stays full window) | if point is in the YX-banded Input | if point is in the YX-banded Input | outside that list, yes | yes | no as requested; Input is rewritten |
| Bounding + Input (same rects) | clipped to the banded silhouette | yes | yes | **no** (glow inside Bounding is clickable) | yes | Bounding+Input stick as the same banded list |
| Bounding > Input (T1 single rects) | yes, inside Bounding | yes, Input | n/a (no VRM) | **yes** | yes | **yes** (`MatchesRequested`) |
| Bounding > Input (T3/T5 unions) | yes (WM expands Bounding to the full client) | yes | yes | **yes** (glow / particle → sink) | yes | Input banded, Bounding reset to full. Usable, not the requested union |
| ShapeNotify reapply | same as T5 | yes | yes | yes | yes | **no** (~151 reapply/s fight) |
| undecorated | yes | yes | yes | yes | yes | same as T5 |
| override_redirect | yes | yes | yes | yes (campaign) | yes | no frame; Input still banded |

---

## Architecture pick (X11)

**B.** Bounding + Input is usable. Visual-only click-through works on
xfwm4. Product code cannot treat Bounding as a stable complex union.

Constraints:

- Prefer **one Bounding AABB** (T1) if the window must clip to the
  visual footprint. Multi-rect Bounding was reset to the full client.
- Input will be YX-banded. Hit tests must tolerate that, or SET a
  single Input AABB.
- Glow / shadow / particles that must be visible have to sit inside
  Bounding. Overflow is clipped (T4).
- Do not reapply Input on ShapeNotify.

Not A: the ideal "Bounding = visual union, Input = interaction union,
both stick as requested" only held for **simple non-overlapping
rects**.

Not C: Input can stay smaller than Bounding. T1 is clean. T5 still
click-throughs visual-only after Bounding becomes the full window.

Not D: SHAPE is WM-dependent, but this is not a reason to invent helper
windows on xfwm4. Helper-window fallback was **not** prototyped.

---

## Final letter

**B. The split basically works. xfwm4 needs X11-specific fallbacks.**

| Criterion | Evidence |
|---|---|
| Cross-process passthrough | T1 Bounding-only and T5 glow/particle: overlay silent, sink `PRESS`/`RELEASE` |
| Visual clipping | T4 particle missing on screen; Clip unused; Bounding is the clip |
| WM interference | YX-band Input; complex Bounding → full window; one rewrite, then stable if you do not fight |
| Dynamic movement | 8–15 SET/s; independent Bounding vs Input dirty flags |
| Performance | SET under 1 ms; 50–100 ms rate limit is enough |
| Complexity / maintainability | One StageWindow. Reapply loop is banned. No helper windows on this WM |

Proceed to a production X11 design that SET Bounding as a visual AABB
(or accept full-window Bounding on compositing WMs) and Input as
interaction, and that treats YX-banding as part of the contract.
Re-measure on Mutter / KWin before calling this "Linux X11."

---

## Pure tests

`build_visual_region` / `build_interaction_region` in
`crates/ene-stage-poc/src/region.rs`. Covered: visual > interaction,
visual-only glow, hidden UI, VRM movement, interaction movement, empty,
multiple components, threshold, independent dirty flags, effective
Input classification.

---

## What this probe did not do

- No production `ene-stage` change
- No Slint port of chat / detail / theme
- No helper-window prototype (B was enough to skip it)
- No Mutter / KWin / Openbox / i3 / bspwm
- No Windows
- Nested Weston `WAYLAND_DISPLAY=ene-poc` must stay unset or winit
  becomes a Wayland client and this probe is invalid
