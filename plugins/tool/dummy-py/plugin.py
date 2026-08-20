#!/usr/bin/env python3
"""Minimal tool.dummy plugin speaking core+tool IPC over a Unix socket."""

from __future__ import annotations

import os
import socket
import struct
import sys
from typing import Any

PLUGIN_ID = "tool.dummy"
PLUGIN_NAME = "dummy"
PLUGIN_VERSION = "0.1.0"


class MsgPack:
  @staticmethod
  def pack(value: Any) -> bytes:
    return bytes(MsgPack._pack(value))

  @staticmethod
  def _pack(value: Any) -> bytearray:
    if value is None:
      return bytearray([0xC0])
    if isinstance(value, bool):
      return bytearray([0xC3 if value else 0xC2])
    if isinstance(value, int):
      if 0 <= value <= 127:
        return bytearray([value])
      if 0 <= value <= 0xFF:
        return bytearray([0xCC, value])
      if 0 <= value <= 0xFFFF:
        return bytearray([0xCD]) + bytearray(struct.pack(">H", value))
      if 0 <= value <= 0xFFFFFFFF:
        return bytearray([0xCE]) + bytearray(struct.pack(">I", value))
      return bytearray([0xCF]) + bytearray(struct.pack(">Q", value))
    if isinstance(value, str):
      data = value.encode("utf-8")
      length = len(data)
      if length <= 31:
        return bytearray([0xA0 | length]) + bytearray(data)
      if length <= 0xFF:
        return bytearray([0xD9, length]) + bytearray(data)
      if length <= 0xFFFF:
        return bytearray([0xDA]) + bytearray(struct.pack(">H", length)) + bytearray(data)
      return bytearray([0xDB]) + bytearray(struct.pack(">I", length)) + bytearray(data)
    if isinstance(value, list):
      length = len(value)
      out = bytearray()
      if length <= 15:
        out.append(0x90 | length)
      elif length <= 0xFFFF:
        out.extend([0xDC])
        out.extend(bytearray(struct.pack(">H", length)))
      else:
        out.extend([0xDD])
        out.extend(bytearray(struct.pack(">I", length)))
      for item in value:
        out.extend(MsgPack._pack(item))
      return out
    if isinstance(value, dict):
      length = len(value)
      out = bytearray()
      if length <= 15:
        out.append(0x80 | length)
      elif length <= 0xFFFF:
        out.extend([0xDE])
        out.extend(bytearray(struct.pack(">H", length)))
      else:
        out.extend([0xDF])
        out.extend(bytearray(struct.pack(">I", length)))
      for key, item in value.items():
        out.extend(MsgPack._pack(str(key)))
        out.extend(MsgPack._pack(item))
      return out
    raise TypeError(f"unsupported type {type(value)!r}")

  @staticmethod
  def unpack(data: bytes) -> Any:
    value, offset = MsgPack._unpack(data, 0)
    if offset != len(data):
      raise ValueError("trailing bytes in frame")
    return value

  @staticmethod
  def _unpack(data: bytes, offset: int) -> tuple[Any, int]:
    if offset >= len(data):
      raise ValueError("unexpected end of input")
    prefix = data[offset]
    offset += 1
    if prefix <= 0x7F:
      return prefix, offset
    if 0xA0 <= prefix <= 0xBF:
      length = prefix - 0xA0
      end = offset + length
      return data[offset:end].decode("utf-8"), end
    if prefix == 0xC0:
      return None, offset
    if prefix == 0xC2:
      return False, offset
    if prefix == 0xC3:
      return True, offset
    if prefix == 0xCC:
      return data[offset], offset + 1
    if prefix == 0xCD:
      return struct.unpack(">H", data[offset : offset + 2])[0], offset + 2
    if prefix == 0xCE:
      return struct.unpack(">I", data[offset : offset + 4])[0], offset + 4
    if prefix == 0xCF:
      return struct.unpack(">Q", data[offset : offset + 8])[0], offset + 8
    if prefix == 0xD9:
      length = data[offset]
      offset += 1
      end = offset + length
      return data[offset:end].decode("utf-8"), end
    if prefix == 0xDA:
      length = struct.unpack(">H", data[offset : offset + 2])[0]
      offset += 2
      end = offset + length
      return data[offset:end].decode("utf-8"), end
    if 0x90 <= prefix <= 0x9F:
      length = prefix - 0x90
      items = []
      for _ in range(length):
        item, offset = MsgPack._unpack(data, offset)
        items.append(item)
      return items, offset
    if prefix == 0xDC:
      length = struct.unpack(">H", data[offset : offset + 2])[0]
      offset += 2
      items = []
      for _ in range(length):
        item, offset = MsgPack._unpack(data, offset)
        items.append(item)
      return items, offset
    if 0x80 <= prefix <= 0x8F:
      length = prefix - 0x80
      mapping: dict[str, Any] = {}
      for _ in range(length):
        key, offset = MsgPack._unpack(data, offset)
        value, offset = MsgPack._unpack(data, offset)
        mapping[str(key)] = value
      return mapping, offset
    if prefix == 0xDE:
      length = struct.unpack(">H", data[offset : offset + 2])[0]
      offset += 2
      mapping = {}
      for _ in range(length):
        key, offset = MsgPack._unpack(data, offset)
        value, offset = MsgPack._unpack(data, offset)
        mapping[str(key)] = value
      return mapping, offset
    raise ValueError(f"unsupported prefix 0x{prefix:02x}")


def read_frame(sock: socket.socket) -> bytes:
  header = recv_exact(sock, 4)
  length = struct.unpack(">I", header)[0]
  return recv_exact(sock, length)


def write_frame(sock: socket.socket, payload: bytes) -> None:
  sock.sendall(struct.pack(">I", len(payload)) + payload)


def recv_exact(sock: socket.socket, size: int) -> bytes:
  chunks: list[bytes] = []
  remaining = size
  while remaining > 0:
    chunk = sock.recv(remaining)
    if not chunk:
      raise ConnectionError("socket closed")
    chunks.append(chunk)
    remaining -= len(chunk)
  return b"".join(chunks)


def tool_specs() -> list[dict[str, Any]]:
  return [
    {
      "name": "dummy.ping",
      "description": "No-op ping",
      "parameters": {"type": "object", "additionalProperties": False},
      "output": {"type": "object"},
      "side_effects": [],
    }
  ]


def handle_call(tool_name: str, args: dict[str, Any]) -> dict[str, Any]:
  if tool_name != "dummy.ping":
    raise ValueError(f"unknown tool {tool_name}")
  return {"pong": args.get("message", "pong")}


def negotiate(hello: dict[str, Any]) -> dict[str, Any]:
  expected = hello.get("expected_digest")
  if not isinstance(expected, str) or not expected:
    raise ValueError("manifest digest mismatch")
  spawn_token = os.environ.get("ENE_PLUGIN_SPAWN_TOKEN")
  if not spawn_token:
    raise RuntimeError("ENE_PLUGIN_SPAWN_TOKEN is not set")
  protocols = hello.get("protocols", {})
  core = protocols.get("core", {})
  tool = protocols.get("tool", {})
  negotiated = {"core": min(core.get("max", 1), 1)}
  if "tool" in protocols:
    negotiated["tool"] = min(tool.get("max", 1), 1)
  return {
    "plugin_id": PLUGIN_ID,
    "plugin_name": PLUGIN_NAME,
    "plugin_version": PLUGIN_VERSION,
    "manifest_digest": expected,
    "spawn_token": spawn_token,
    "protocols": negotiated,
  }


def serve() -> None:
  socket_path = os.environ.get("ENE_PLUGIN_SOCKET")
  if not socket_path:
    raise RuntimeError("ENE_PLUGIN_SOCKET is not set")
  if os.name == "nt":
    host, port = socket_path.rsplit(":", 1)
    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    sock.connect((host, int(port)))
  else:
    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    sock.connect(socket_path)
  try:
    hello = MsgPack.unpack(read_frame(sock))
    if hello.get("kind") != "hello":
      raise ValueError("expected hello")
    write_frame(sock, MsgPack.pack({"kind": "hello_ack", "body": negotiate(hello["body"])}))
    while True:
      message = MsgPack.unpack(read_frame(sock))
      kind = message.get("kind")
      msg_id = message.get("id")
      if kind == "tool_list":
        write_frame(sock, MsgPack.pack({"kind": "tool_spec", "id": msg_id, "tools": tool_specs()}))
      elif kind == "tool_call":
        body = message["body"]
        try:
          value = handle_call(body["tool_name"], body.get("args", {}))
          result = {"call_id": body["call_id"], "status": "ok", "value": value}
        except Exception as exc:  # noqa: BLE001
          result = {"call_id": body["call_id"], "status": "error", "value": {"error": str(exc)}}
        write_frame(sock, MsgPack.pack({"kind": "tool_result", "id": msg_id, "body": result}))
      elif kind == "ping":
        write_frame(sock, MsgPack.pack({"kind": "pong", "id": msg_id}))
      elif kind in {"drain", "shutdown"}:
        write_frame(sock, MsgPack.pack({"kind": "drain_ack", "id": msg_id}))
        return
      else:
        raise ValueError(f"unexpected message {kind!r}")
  finally:
    sock.close()


def main() -> None:
  try:
    serve()
  except Exception as exc:  # noqa: BLE001
    print(f"dummy plugin failed: {exc}", file=sys.stderr)
    sys.exit(1)


if __name__ == "__main__":
  main()
