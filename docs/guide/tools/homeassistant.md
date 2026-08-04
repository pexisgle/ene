# Home Assistant Tool Guide

`ene-plugin-homeassistant` provides smart home control through the
[Home Assistant REST API](https://developers.home-assistant.io/docs/api/rest/):
entity state reads, switch/light/plug control, and climate temperature
setting. It is a built-in tool plugin and starts automatically on fresh
installs.

## Configuration

The plugin talks to your own Home Assistant instance, so it needs a base URL
and a long-lived access token before any action works:

- `base_url` — your instance URL, e.g. `http://homeassistant.local:8123`.
  A reverse-proxy path prefix is supported and must end with `/` (e.g.
  `https://home.example.com/ha/`). Only `http` and `https` are accepted.
- `token` — a long-lived access token created in Home Assistant under
  **Profile → Security → Long-Lived Access Tokens**.

> **Security:** over plaintext `http://` the long-lived token is sent
> unencrypted on your network for every request, and anyone on the same
> network segment can capture it and control your smart home. Prefer
> `https://` (e.g. Home Assistant behind a TLS reverse proxy), or at least
> restrict `http://` access to a trusted network. Ene logs a startup
> warning whenever the configured base URL uses `http`.

Set them in `settings.json` under `plugins.list.homeassistant.config`:

```json
{
  "plugins": {
    "list": {
      "homeassistant": {
        "enable": true,
        "config": {
          "base_url": "http://homeassistant.local:8123",
          "token": "your-long-lived-token"
        }
      }
    }
  }
}
```

The token field is marked `x-ene-secret` in the plugin's config schema, so
Ene redacts it from host logs; settings-UI masking of secret fields is
planned. The same value can be set with the environment override
`ENE_PLUGINS__LIST__HOMEASSISTANT__CONFIG__TOKEN`.

The plugin also declares the credential id `homeassistant` (private, kind
`api_key`) via `x-ene-credentials`, ready for Ene's credential helper to
deliver the stored token once the host-side credential client API reaches
the mainline; until then the token is read from plugin config as above.

## Actions

### `homeassistant.state`

Reads the current state, attributes, and last-updated time of an entity:

```json
{"entity_id": "light.living_room"}
```

```json
{"entity_id": "sensor.outdoor_temperature"}
```

This action is read-only and runs without an approval prompt. It does reach
your Home Assistant instance over the network, so the transport security
notes above apply to it as well.

### `homeassistant.turn_on` / `homeassistant.turn_off`

Turns a switch, light, smart plug, or other on/off entity on or off:

```json
{"entity_id": "switch.kitchen_plug"}
```

These actions change the physical state of a device, so they require
explicit user approval before anything is sent to Home Assistant. The
approval prompt shows the exact entity id and the intended action.
Approvals last for the current turn; choosing "allow for this session"
keeps the permission for the rest of the conversation. Direct tool calls
(`ene-cli tool call`) run under a fresh synthetic turn, so they never
inherit approvals granted during a chat.

### `homeassistant.set_temperature`

Sets the target temperature of a climate entity (air conditioner, heater,
thermostat):

```json
{"entity_id": "climate.living_room", "temperature": 22.0}
```

An optional HVAC mode can be applied together with the temperature:

```json
{"entity_id": "climate.living_room", "temperature": 18.0, "hvac_mode": "heat"}
```

Allowed modes: `off`, `heat`, `cool`, `heat_cool`, `auto`, `dry`,
`fan_only`. Like the turn actions, this changes physical state and requires
explicit user approval.

## Troubleshooting

- Entity ids are strictly `domain.entity` with lowercase letters, digits,
  and underscores — `light.living_room` is valid, `Living Room` is not.
- Requests time out after 10 seconds; if Home Assistant is on a slow network
  or behind a reverse proxy, check the base URL first.
- Home Assistant error bodies (`{"code", "message"}`) are passed through in
  tool errors, so a rejected service call explains itself.
- The token is never included in tool results or error messages; if you see
  HTTP 401, the token is missing, expired, or was revoked in Home Assistant.
