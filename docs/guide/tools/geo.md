# Geo Tool Guide

`ene-plugin-geo` provides geographic information tools: IP-based location,
current weather, solar timezone offset calculation, and sunrise/sunset
times. It is a built-in tool plugin and starts automatically on fresh
installs.

## Actions

### `geo.location`

Looks up the approximate geographic location of an IP address via
[ip-api.com](https://ip-api.com/) — country, region, city, coordinates, and
timezone.

```json
{"ip": "8.8.8.8"}
```

When `ip` is omitted, the caller's own public IP is located:

```json
{}
```

Locating the caller's own IP reveals the user's approximate location, so
that call requires explicit user approval. Approvals are per-turn; choosing
"allow for this session" keeps the permission for the rest of the
conversation.

### `geo.weather`

Returns current weather conditions (temperature, humidity, cloud cover,
wind, visibility, pressure) from [wttr.in](https://wttr.in/). `location` is
a city name or `lat,lon` coordinates:

```json
{"location": "Tokyo"}
```

```json
{"location": "35.68,139.69"}
```

When `location` is omitted, wttr.in derives the location from the caller's
IP address. Like `geo.location`, that call reveals the user's approximate
location and requires explicit user approval.

### `geo.timezone`

Calculates the theoretical solar UTC offset for a longitude (15 degrees of
longitude per hour) — no external API is involved:

```json
{"longitude": 139.68}
```

The result is the solar offset, not the civil timezone: political
timezone boundaries and daylight saving time are not reflected.

### `geo.sunrise_sunset`

Returns sunrise, sunset, solar noon, day length, and twilight times for
coordinates from [sunrise-sunset.org](https://sunrise-sunset.org/). An
optional date (`YYYY-MM-DD`; when omitted, the tool computes today's date in
UTC and always sends it explicitly, since the service's own no-date default
is its server-local day) and IANA timezone name (default `UTC`) select the
reference day and the offset of the returned timestamps:

```json
{"latitude": 35.68, "longitude": 139.69, "date": "2026-08-04", "tzid": "Asia/Tokyo"}
```

## Privacy and network notes

- `geo.location` and IP-derived `geo.weather` send the user's public IP to a
  third-party service and reveal the approximate location. Both are gated
  behind the same per-turn approval mechanism used by write operations
  elsewhere in Ene.
- ip-api.com's free tier only supports plain HTTP, so the `geo.location`
  request itself is not encrypted (the IP is already visible to any HTTP
  endpoint the machine talks to, but the network path sees it too).
- All HTTP calls use a 10 second timeout and response bodies are capped at
  1 MiB; outputs contain only a fixed set of fields, so results stay small.
- `geo.timezone` never leaves the machine.

## Configuration

No configuration is required. The plugin is enabled by default
(`tools.list.geo.enable = true` in `settings.json`) and can be disabled like
any other tool plugin.
