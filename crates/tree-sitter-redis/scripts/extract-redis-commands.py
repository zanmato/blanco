#!/usr/bin/env python3
"""Regenerate grammar/keywords.js from a live Redis server.

Speaks RESP directly (no redis-cli dependency) and queries `COMMAND LIST` for
the full command set plus `COMMAND DOCS` for container commands' subcommands.
By default it targets the dev server in docker-compose.yml.

    python3 scripts/extract-redis-commands.py --host 127.0.0.1 --port 6400 --password blanco
"""

import argparse
import json
import os
import socket


class RespClient:
    def __init__(self, host, port, password):
        self.s = socket.create_connection((host, port), timeout=10)
        self.buf = b""
        if password:
            reply = self.cmd("AUTH", password)
            if reply != "OK":
                raise RuntimeError(f"AUTH failed: {reply!r}")

    def _send(self, *args):
        out = f"*{len(args)}\r\n".encode()
        for arg in args:
            arg = str(arg).encode()
            out += b"$" + str(len(arg)).encode() + b"\r\n" + arg + b"\r\n"
        self.s.sendall(out)

    def _read_line(self):
        while b"\r\n" not in self.buf:
            self.buf += self.s.recv(65536)
        line, self.buf = self.buf.split(b"\r\n", 1)
        return line

    def _read_n(self, n):
        while len(self.buf) < n + 2:
            self.buf += self.s.recv(65536)
        data = self.buf[:n]
        self.buf = self.buf[n + 2 :]
        return data

    def _parse(self):
        line = self._read_line()
        kind, rest = chr(line[0]), line[1:]
        if kind in "+-":
            return rest.decode(errors="replace")
        if kind == ":":
            return int(rest)
        if kind == "$":
            length = int(rest)
            return None if length == -1 else self._read_n(length).decode(errors="replace")
        if kind in "*~>":
            count = int(rest)
            return None if count == -1 else [self._parse() for _ in range(count)]
        if kind == "%":
            count = int(rest)
            return {self._parse(): self._parse() for _ in range(count)}
        if kind == "_":
            return None
        return rest.decode(errors="replace")

    def cmd(self, *args):
        self._send(*args)
        return self._parse()


def as_map(value):
    if isinstance(value, dict):
        return value
    return dict(zip(value[::2], value[1::2]))


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=6400)
    parser.add_argument("--password", default="blanco")
    args = parser.parse_args()

    client = RespClient(args.host, args.port, args.password)

    names = [n.upper() for n in client.cmd("COMMAND", "LIST") if "|" not in n]

    subcommands = {}
    for name, attrs in as_map(client.cmd("COMMAND", "DOCS")).items():
        subs = as_map(attrs).get("subcommands")
        if not subs:
            continue
        full_names = subs.keys() if isinstance(subs, dict) else subs[::2]
        sub_names = sorted({full.split("|", 1)[1].upper() for full in full_names if "|" in full})
        if sub_names:
            subcommands[name.upper()] = sub_names

    containers = sorted(subcommands)
    container_set = set(containers)
    commands = sorted(n for n in set(names) if n not in container_set)
    flat_subcommands = sorted({s for subs in subcommands.values() for s in subs})

    def render_array(name, items):
        body = ",\n  ".join(json.dumps(item) for item in items)
        return f"const {name} = [\n  {body},\n];\n"

    here = os.path.dirname(__file__)
    header = (
        "// Generated from `COMMAND LIST` / `COMMAND DOCS` against redis:latest (Redis 8).\n"
        "// Regenerate with scripts/extract-redis-commands.py. Do not edit by hand.\n\n"
    )

    # Grammar keyword include (CommonJS, consumed by grammar.js).
    js_path = os.path.join(here, "..", "grammar", "keywords.js")
    with open(js_path, "w") as out:
        out.write(header)
        out.write(render_array("COMMANDS", commands))
        out.write("\n")
        out.write(render_array("CONTAINERS", containers))
        out.write("\n")
        out.write(render_array("SUBCOMMANDS", flat_subcommands))
        out.write("\nmodule.exports = { COMMANDS, CONTAINERS, SUBCOMMANDS };\n")

    # Rust catalog, used by the editor's Redis completion provider. COMMANDS here
    # is the full top-level set (plain + container commands) so it can drive
    # first-token completion directly; SUBCOMMANDS maps each container to its
    # subcommands for second-token completion.
    all_commands = sorted(set(commands) | container_set)

    def render_rust_array(items):
        return "".join(f"    {json.dumps(item)},\n" for item in items)

    rs_path = os.path.join(here, "..", "bindings", "rust", "commands.rs")
    with open(rs_path, "w") as out:
        out.write(header)
        out.write("/// Every top-level Redis command name (uppercase), sorted.\n")
        out.write("pub const COMMANDS: &[&str] = &[\n")
        out.write(render_rust_array(all_commands))
        out.write("];\n\n")
        out.write("/// Subcommands for each container command, keyed by the uppercase\n")
        out.write("/// container name. Both the outer list and each inner list are sorted.\n")
        out.write("pub const SUBCOMMANDS: &[(&str, &[&str])] = &[\n")
        for name in containers:
            subs = ", ".join(json.dumps(s) for s in subcommands[name])
            out.write(f"    ({json.dumps(name)}, &[{subs}]),\n")
        out.write("];\n")

    print(f"commands={len(commands)} containers={len(containers)} subcommands={len(flat_subcommands)}")


if __name__ == "__main__":
    main()
