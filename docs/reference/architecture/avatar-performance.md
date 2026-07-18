# Avatar Performance System

- **Status:** Accepted
- **Date:** 2026-07-14
- **Epic:** [#126](https://github.com/pexisgle/ene/issues/126)
- **Sub-issues:** [#128](https://github.com/pexisgle/ene/issues/128) (marker grammar), [#129](https://github.com/pexisgle/ene/issues/129) (arbiter), [#130](https://github.com/pexisgle/ene/issues/130) (motion catalog), [#131](https://github.com/pexisgle/ene/issues/131) (LayerComposer), [#132](https://github.com/pexisgle/ene/issues/132) (expression compositor), [#133](https://github.com/pexisgle/ene/issues/133) (desktop wiring), [#134](https://github.com/pexisgle/ene/issues/134) (prompt contract)

## Overview

The performance system unifies expression, motion, and look-at control under a single `PerformanceCue` type. Cues originate from three sources (LLM stream markers, affect engine, character hysteresis) and are arbitrated by priority before reaching the VRM renderer.

## Architecture

```
Text stream ──▶ parse_performance_marker() ──▶ PerformanceArbiter
                     │                              │
Affect engine ───────┼──────────────────────────────┤
                     │                              │
Character config ────┼──────────────────────────────┤
  (motion catalog,   │                              ▼
   expressions)     │                     resolve() → Vec<(PerformanceCue, CueSource)>
                     │                              │
                     ▼                              ▼
               LayerComposer  ◀────────  EneEvent::Performance
               ExpressionCompositor
                     │
                     ▼
               VRM renderer (blendshapes, bone animations, look-at)
```

## Data Flow

### 1. Stream Marker Parsing (`#128`)

The LLM may emit `<|perf:…|>` tokens inline in streaming text. `<|emo:NAME|>` is the shorthand expression form of `<|perf:expr=NAME|>`.

| Marker | Example | Effect |
|--------|---------|--------|
| Expression (shorthand) | `<\|emo:happy\|>` | Set expression blend-shape |
| Expression | `<\|perf:expr=happy\|>` | Set expression blend-shape |
| Expression (weighted) | `<\|perf:expr=happy,weight=0.8,hold=2.0\|>` | With weight and hold duration |
| Motion | `<\|perf:motion=wave,layer=upper\|>` | Play upper-body motion |
| Look-at | `<\|perf:lookat=user\|>` | Glance at target |
| Cancel | `<\|perf:cancel=expr\|>` / `<\|perf:cancel=motion\|>` / `<\|perf:cancel=all\|>` | Clear slot |

See `ene_mind::session::special_token::parse_performance_marker()` and `extract_emotion_from_token()`.

### 2. Performance Arbiter (`#129`)

`PerformanceArbiter` collects cues during a turn and resolves the final set at turn-end.

**Priority order (higher wins):**
| Priority | Source | Value |
|----------|--------|-------|
| 5 | `LlmCommand` | LLM token, `llm_expression_is_advisory = false` |
| 4 | `LlmAdvisory` | LLM token, advisory mode on |
| 3 | `Affect` | Affect-to-expression mapping |
| 2 | `Hysteresis` | Previous expression held |
| 1 | `Fallback` | Neutral or nearest |

- Same priority: latest wins
- Different cue kinds (expression, motion, look-at) are independent slots
- Cancel cues clear the matching slot(s)

See `ene_mind::output::performance_arbiter::PerformanceArbiter`.

### 3. Motion Catalog (`#130`)

Character cards declare available motions under `extensions.ene`:

```json
{
  "extensions": {
    "ene": {
      "motion_catalog": {
        "idle_lower": "idle_basic",
        "motions": [
          { "name": "wave",   "path": "motions/wave.vrma",   "layer": "upper" },
          { "name": "idle_basic", "path": "motions/idle.vrma", "layer": "lower" }
        ]
      }
    }
  }
}
```

- `MotionLayer::Upper` — upper-body gesture (arms, head), coexists with lower
- `MotionLayer::Lower` — lower-body idle loop, coexists with upper
- `MotionLayer::Full` — full-body override, preempts upper and lower
- `idle_lower` — default idle loop name for desktop rendering

### 4. Layer Composer (`#131`)

`LayerComposer` manages three motion layers:

```
Full ──▶ preempts Upper + Lower
Upper ──▶ coexists with Lower (upper-body gestures)
Lower ──▶ coexists with Upper (idle loop)
```

See `ene_vrm::layer_composer::LayerComposer`.

### 5. Expression Compositor (`#132`)

`ExpressionCompositor` merges card-defined expression weights with runtime overrides:

1. **Base layer** — card expression blendshape map (e.g. `"happy" → {"happy": 0.8, "mouthSmile": 0.5}`)
2. **Override layer** — per-blendshape runtime overrides from `PerformanceCue`
3. **Compose** — base ∪ overrides, overrides win on matching keys

See `ene_vrm::expression_compositor::ExpressionCompositor`.

### 6. Desktop Wiring (`#133`)

- `EneEvent::Performance` → `ai_bridge.rs` routes to `AppEvent::PerformanceCue` (expressions) or `AppEvent::MotionCue` (motions)
- Expression path: `EmotionPipelineState` (hold/fade) → VRM `ExpressionsLayer`
- Motion path: `MotionLayerState` (wraps `LayerComposer`) → VRM `VrmaPlayer`
- `cue_source_to_u8()` maps `CueSource` to integer priority

## Prompt Contract (`#134`)

When the emotion engine is **disabled**, the LLM receives performance grammar instructions in the post-history PHI block (replacing the old `<|emo:NAME|>` contract):

```
## Performance Output Rule
RULE: You may use special tokens to control your avatar's expression, motion, and gaze.
Place each token BEFORE the sentence it describes.

Grammar:
  Expression (required, shorthand): `<|emo:NAME|>`
  Expression (full): `<|perf:expr=NAME[,weight=0.0-1.0][,hold=SECS]|>`
  Motion: `<|perf:motion=NAME[,layer=upper|lower|full]|>`
  Look-at: `<|perf:lookat=TARGET|>`
  Cancel: `<|perf:cancel=expr|motion|all|>`

Available expressions:
- `<|perf:expr=happy|>` — Feeling joyful, excited, or pleased
...
```

When the emotion engine is **enabled**, the natural-dialogue contract instructs the LLM to use plain text only — the runtime manages expression from affect state.

## Crate Ownership

| Type | Crate | Notes |
|------|-------|-------|
| `PerformanceCue`, `CueSource`, `PerfKind`, `MotionLayer` (cue) | `ene-mind` | Re-exported by `ene-runtime` |
| `PerformanceArbiter` | `ene-mind` | Mid-turn arbitration |
| `MotionLayer` (config), `MotionCatalog`, `MotionEntry` | `ene-config` | Serialization on character card |
| `LayerComposer`, `MotionLayer` (vrm) | `ene-vrm` | Independent of mind/runtime |
| `ExpressionCompositor` | `ene-vrm` | Expression weight merging |

## Related Documents

- [Emotions](../runtime/emotions.md) — Affect engine and expression mapping
- [Streaming Events](../runtime/streaming-events.md) — Chat event bus
- [API v1 ADR](api-v1.md) — Host contract and crate map
