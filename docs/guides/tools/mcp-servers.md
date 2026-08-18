# MCP servers

[Model Context Protocol](https://modelcontextprotocol.io) servers are not
vendored. v1.0 connects them as handwritten profile rows; they then run
through the same `ene-registry` pipeline as in-tree tools.

There is no settings-page picker yet (successor, P-616). Do not add a
`tools.mcp_servers` block copied from the old in-process host — that schema
is gone.
