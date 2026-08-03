# Random Generation Tool Guide

The built-in `random` tool plugin generates random numbers, UUIDs, list
picks, and hex colors. Every action is a pure computation: it reads no
state, never touches the network or file system, and is available as
soon as the plugin is enabled (it is part of the default `plugins.list`).

## Actions

| Action | Description |
|---|---|
| `random.number` | Random number within a range (float or integer) |
| `random.uuid` | Random UUID (version 4) |
| `random.pick` | Random element from a list |
| `random.color` | Random color as a hex string |

## `random.number`

Returns a random number between `min` and `max`.

| Argument | Type | Default | Description |
|---|---|---|---|
| `min` | number | `0` | Lower bound; must be finite |
| `max` | number | `1` | Upper bound; must be finite |
| `integer` | boolean | `false` | When `true`, return a whole number instead of a float |

With `integer: false` the result is a float in the half-open interval
`[min, max)` — `min` is included, `max` is excluded, and `min` must be
less than `max`. With `integer: true` the result is a whole number in
the closed interval `[min, max]`; fractional bounds are rounded inward
(`ceil(min)` to `floor(max)`), and a range containing no whole numbers
is an error.

```json
{ "min": 1, "max": 6, "integer": true }
```

## `random.uuid`

Takes no arguments. Returns a version 4 (random) UUID in canonical
hyphenated lowercase form:

```json
{}
```

→ `550e8400-e29b-41d4-a716-446655440000`

## `random.pick`

Selects one element uniformly at random from `options` and returns it as
a string.

| Argument | Type | Description |
|---|---|---|
| `options` | string[] | List to pick from; must contain at least one element |

```json
{ "options": ["rock", "paper", "scissors"] }
```

## `random.color`

Takes no arguments. Returns a random color as a lowercase CSS hex string
(`#rrggbb`) with uniformly random red, green, and blue channels:

```json
{}
```

→ `#3f8ab2`

## Notes

- All actions declare `ReadOnly` side effects: they never mutate host
  state, so no permission prompts are triggered.
- The plugin takes no configuration; there is nothing to set up in
  `plugins.list` beyond enabling it.
- Randomness comes from the system CSPRNG (`rand`), so results are
  suitable for identifiers and picks, not for security-critical keys —
  use a dedicated key-generation mechanism for those.
