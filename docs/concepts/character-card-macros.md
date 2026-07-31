# Character Card Macros (CBS)

Character cards (CCv3) may embed **CBS template macros** — `{{…}}` placeholders
that Ene expands when it compiles the card into a prompt. Macros let card
authors reference the character and user by name, inject card fields, vary
traits, and react to the time of day without hard-coding values.

Expansion runs at **identity-kernel compile time**, which happens once per
turn. A macro's output is therefore baked into the prompt for that turn.

---

## Supported macros

| Macro | Expands to | Notes |
|---|---|---|
| `{{char}}`, `<char>`, `<bot>` | Character display name | `nickname` is preferred over `name` when set |
| `{{user}}` | User display name | |
| `{{user_persona}}` | Structured user-persona block | Name / description / relationship / pronouns / notes; removed when no persona is configured |
| `{{description}}` | Card `description` field | |
| `{{personality}}` | Card `personality` field | |
| `{{scenario}}` | Card `scenario` field | |
| `{{persona}}` | Card `creator_notes` field | |
| `{{mesExamples}}` | Card `mes_example` field | |
| `{{random:a,b,c}}` | One option, chosen at random | **Re-rolled on every evaluation** |
| `{{pick:a,b,c}}` | One option, chosen stably | **Stable within a chat** — see below |
| `{{roll:d20}}` | A dice roll in `1..=N` | `d` prefix optional; re-rolled every evaluation |
| `{{time}}` | Local time, `HH:MM` | |
| `{{date}}` | Local date, `YYYY/MM/DD` | |
| `{{isotime}}` | Local time, `HH:MM:SS` | |
| `{{isodate}}` | Local date, `YYYY-MM-DD` | |
| `{{weekday}}` | English weekday name | e.g. `Saturday` |
| `{{idle_duration}}` | Human-friendly idle span | e.g. `just now`, `45 minutes`, `3 hours`, `2 days` |
| `{{// …}}`, `{{comment:…}}` | *(removed)* | Author comments, stripped from the prompt |
| `{{reverse:text}}` | `text` reversed | |

---

## `{{random}}` vs `{{pick}}`

The two selection macros look alike but behave differently, matching the wider
CBS ecosystem:

- **`{{random:a,b,c}}`** draws a fresh option **every time** it is evaluated.
  Use it for flavour that should vary turn to turn.
- **`{{pick:a,b,c}}`** resolves to a **stable** option for the lifetime of a
  chat. The choice is derived deterministically from the option text plus a
  per-session seed (character + session id), so a trait chosen once — hair
  colour, hometown, favourite food — does **not** change when the identity
  kernel is recompiled on later turns.

Because the identity kernel is recompiled on every turn, treating `{{pick}}`
as random (the historical behaviour) made character traits flicker between
turns. `{{pick}}` is now seeded so the same chat always sees the same choice;
a different chat (different session) may pick differently.

---

## Time macros

The time macros expand against the **local** system clock at compile time:

- `{{time}}` / `{{isotime}}` — time of day.
- `{{date}}` / `{{isodate}}` — calendar date.
- `{{weekday}}` — English weekday name.
- `{{idle_duration}}` — how long since the user was last active, phrased
  coarsely (`just now`, `N minutes`, `N hours`, `N days`). When no activity
  record is available the macro is left unexpanded rather than reporting a
  misleading zero.

These enable time-aware card writing, e.g. greeting the user differently in
the morning or reacting after a long silence.

---

## Not implemented

Control-flow macros from some CBS dialects (RisuAI-style `{{#if}}`, arithmetic,
conditionals) are intentionally **not** supported. Unknown macros are left in
the text untouched, so a typo surfaces visibly rather than vanishing.
