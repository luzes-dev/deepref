#!/usr/bin/env python3
"""Bounded fault injection against exact owned disposable Docker fixtures.

CLI requires the inherited global fixture flock on descriptor 9. It derives the
loopback destination from the verified container; it accepts no database URL or
arbitrary upstream port. The caller owns service creation, evidence and cleanup.
Socket evidence from this tool is not consequential database recovery evidence.
"""

import argparse
import asyncio
import contextlib
import errno
import fcntl
import hmac
import json
import os
from pathlib import Path
import re
import secrets
import signal
import stat
import subprocess
from dataclasses import dataclass

MAX_CONNECTIONS = 16
MAX_CONTROL_CONNECTIONS = 4
READ_BYTES = 65536
CONNECTION_SECONDS = 30
CONTROL_BYTES = 4096
CONTROL_SECONDS = 2
MAX_LIFETIME_SECONDS = 300
FIXTURE_LOCK = Path.home() / ".cache/deepref-impl/cargo-build.lock"
MODES = frozenset({"pass", "drop", "stall_response"})


@dataclass(frozen=True)
class Destination:
    port: int
    host: str = "127.0.0.1"

    def __post_init__(self):
        if self.host not in {"127.0.0.1", "::1"}:
            raise ValueError("fault proxy upstream must be literal loopback")
        if type(self.port) is not int or not 1 <= self.port <= 65535:
            raise ValueError("invalid fixture port")


@dataclass(frozen=True)
class OwnedFixture:
    container_id: str
    name: str
    task_label: str
    container_port: int

    def __post_init__(self):
        if not re.fullmatch(r"[0-9a-f]{64}", self.container_id):
            raise ValueError("exact container ID required")
        if not re.fullmatch(r"deepref-ops-[a-z0-9-]{8,80}", self.name):
            raise ValueError("owned operations fixture name required")
        if not re.fullmatch(r"operations-[a-z0-9-]{8,80}", self.task_label):
            raise ValueError("unique operations task label required")
        Destination(self.container_port)

    def destination(self, inspected):
        """Validate the selected Docker catalog boundary, never an app URL."""
        if not isinstance(inspected, dict) or (
            inspected.get("id") != self.container_id
            or inspected.get("name") != "/" + self.name
            or inspected.get("task") != self.task_label
            or inspected.get("running") is not True
            or inspected.get("paused") is not False
        ):
            raise ValueError("owned fixture identity/state mismatch")
        bindings = inspected.get("binding")
        if not isinstance(bindings, list) or len(bindings) != 1:
            raise ValueError("one explicit loopback published binding required")
        binding = bindings[0]
        if not isinstance(binding, dict) or binding.get("HostIp") != "127.0.0.1":
            raise ValueError("fixture port must bind only to 127.0.0.1")
        port = binding.get("HostPort")
        if not isinstance(port, str) or not re.fullmatch(r"[0-9]{1,5}", port):
            raise ValueError("invalid published fixture port")
        return Destination(int(port))

    def inspect(self):
        # Only fixed identity/port fields; never capture environment or payloads.
        template = (
            '{"id":{{json .Id}},"name":{{json .Name}},'
            '"running":{{json .State.Running}},"paused":{{json .State.Paused}},'
            '"task":{{json (index .Config.Labels "deepref.task")}},'
            '"binding":{{json (index .NetworkSettings.Ports "'
            + str(self.container_port) + '/tcp")}}}'
        )
        result = subprocess.run(
            ["docker", "inspect", "--format", template, self.container_id],
            check=True, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
            timeout=5, text=True,
        )
        if len(result.stdout) > 4096:
            raise ValueError("unexpected fixture catalog size")
        return self.destination(json.loads(result.stdout))


def require_held_lock(path, descriptor):
    """Require this inherited open description to hold the exact flock inode."""
    own = os.fstat(descriptor)
    with open(path, "rb") as probe:
        expected = os.fstat(probe.fileno())
        if not stat.S_ISREG(own.st_mode) or (own.st_dev, own.st_ino) != (
            expected.st_dev, expected.st_ino
        ):
            raise ValueError("inherited fixture flock inode mismatch")
        try:
            fcntl.flock(probe.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError as error:
            if error.errno not in {errno.EACCES, errno.EAGAIN}:
                raise
        else:
            fcntl.flock(probe.fileno(), fcntl.LOCK_UN)
            raise ValueError("fixture flock is not held")
        # A lock held by someone else is insufficient. This succeeds only for
        # the same inherited open-file description, and leaves the lock held.
        fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)


class FaultProxy:
    def __init__(self, upstream: Destination, lifetime_seconds=MAX_LIFETIME_SECONDS):
        if type(lifetime_seconds) is not int or not 1 <= lifetime_seconds <= MAX_LIFETIME_SECONDS:
            raise ValueError("proxy lifetime must be 1..300 seconds")
        self.upstream = upstream
        self.lifetime_seconds = lifetime_seconds
        self._token = secrets.token_urlsafe(32)
        self._mode = "pass"
        self._writers = set()
        self._clients = set()
        self._upstreams = set()
        self._failed = False
        self._tasks = set()
        self._connections = 0
        self._controls = 0
        self._resume = asyncio.Event()
        self._resume.set()
        self._expiry = None
        self._lifetime = None
        self._servers = []
        self._closing = False
        self._closed = asyncio.Event()

    async def start(self):
        if self._servers or self._closing:
            raise RuntimeError("proxy cannot be started twice")
        try:
            for callback, read_limit in ((self._accept, READ_BYTES), (self._control, CONTROL_BYTES)):
                self._servers.append(await asyncio.start_server(
                    callback, "127.0.0.1", 0, limit=read_limit,
                    backlog=MAX_CONNECTIONS,
                ))
            self._lifetime = asyncio.get_running_loop().call_later(
                self.lifetime_seconds, lambda: asyncio.create_task(self.close())
            )
        except BaseException:
            await self.close()
            raise
        return {
            "proxy_port": self._servers[0].sockets[0].getsockname()[1],
            "control_port": self._servers[1].sockets[0].getsockname()[1],
            "control_token": self._token,
            "lifetime_seconds": self.lifetime_seconds,
        }

    def _admit(self, reader, writer, control):
        count, limit = (self._controls, MAX_CONTROL_CONNECTIONS) if control else (
            self._connections, MAX_CONNECTIONS
        )
        if self._closing or count >= limit or (not control and self._mode == "drop"):
            writer.transport.abort()
            return
        writer.transport.set_write_buffer_limits(high=READ_BYTES, low=READ_BYTES // 4)
        self._writers.add(writer)
        if control:
            self._controls += 1
        else:
            self._connections += 1
            self._clients.add(writer)
        task = asyncio.create_task(self._command(reader, writer) if control else self._client(reader, writer))
        self._tasks.add(task)

        def finished(done):
            self._tasks.discard(done)
            self._writers.discard(writer)
            if control:
                self._controls -= 1
            else:
                self._connections -= 1
            writer.transport.abort()
            if not done.cancelled() and done.exception() is not None:
                # No payload or exception details enter diagnostics.
                self._failed = True
                asyncio.create_task(self.close())

        task.add_done_callback(finished)

    def _accept(self, reader, writer):
        self._admit(reader, writer, False)

    def _control(self, reader, writer):
        self._admit(reader, writer, True)

    def _set_fault(self, mode, seconds):
        if not isinstance(mode, str) or mode not in MODES or type(seconds) is not int:
            raise ValueError("invalid fault")
        if not 1 <= seconds <= 30:
            raise ValueError("fault duration must be 1..30 seconds")
        if self._expiry:
            self._expiry.cancel()
        self._mode = mode
        if mode == "stall_response":
            self._resume.clear()
        else:
            self._resume.set()
        if mode == "drop":
            # Control acknowledgement remains writable; only data paths drop.
            for writer in tuple(self._data_writers):
                writer.transport.abort()
        self._expiry = asyncio.get_running_loop().call_later(seconds, self._restore)

    def _restore(self):
        self._mode = "pass"
        self._resume.set()
        self._expiry = None

    @property
    def _data_writers(self):
        return self._clients | self._upstreams

    async def _relay(self, reader, writer, response):
        while data := await reader.read(READ_BYTES):
            if response:
                await self._resume.wait()
            writer.write(data)
            await writer.drain()
        if writer.can_write_eof():
            writer.write_eof()
            await writer.drain()

    async def _client(self, reader, writer):
        try:
            async with asyncio.timeout(CONNECTION_SECONDS):
                upstream_reader, upstream_writer = await asyncio.open_connection(
                    self.upstream.host, self.upstream.port, limit=READ_BYTES
                )
                upstream_writer.transport.set_write_buffer_limits(high=READ_BYTES, low=READ_BYTES // 4)
                self._upstreams.add(upstream_writer)
                try:
                    if self._mode == "drop" or self._closing:
                        return
                    relays = [asyncio.create_task(self._relay(reader, upstream_writer, False)),
                              asyncio.create_task(self._relay(upstream_reader, writer, True))]
                    try:
                        await asyncio.gather(*relays)
                    finally:
                        for relay in relays:
                            relay.cancel()
                        await asyncio.gather(*relays, return_exceptions=True)
                finally:
                    upstream_writer.transport.abort()
                    self._upstreams.discard(upstream_writer)
        except (TimeoutError, ConnectionError, OSError):
            pass
        finally:
            self._clients.discard(writer)

    async def _command(self, reader, writer):
        try:
            async with asyncio.timeout(CONTROL_SECONDS):
                line = await reader.readline()
                if not line.endswith(b"\n") or len(line) > CONTROL_BYTES:
                    raise ValueError("bounded line required")
                request = json.loads(line)
                if not isinstance(request, dict) or set(request) != {"token", "mode", "seconds"}:
                    raise ValueError("expected token, mode and seconds only")
                token = request["token"]
                if not isinstance(token, str) or not hmac.compare_digest(token, self._token):
                    raise ValueError("invalid control token")
                self._set_fault(request["mode"], request["seconds"])
                result = {"ok": True, "mode": self._mode, "connections": self._connections}
                writer.write(json.dumps(result).encode() + b"\n")
                await writer.drain()
        except (TimeoutError, ConnectionError, OSError, ValueError, TypeError, RecursionError):
            with contextlib.suppress(ConnectionError, OSError):
                writer.write(b'{"ok":false,"error":"invalid bounded fault command"}\n')
                await writer.drain()
        finally:
            writer.close()
            with contextlib.suppress(TimeoutError, ConnectionError, OSError):
                await asyncio.wait_for(writer.wait_closed(), 1)

    async def wait_closed(self):
        await self._closed.wait()
        if self._failed:
            raise RuntimeError("fault proxy task failed")

    async def close(self):
        if self._closing:
            await self._closed.wait()
            return
        self._closing = True
        try:
            for timer in (self._expiry, self._lifetime):
                if timer:
                    timer.cancel()
            for server in self._servers:
                server.close()
            # Server.wait_closed also waits for accepted transports. Abort
            # owned clients before awaiting it, including a stalled response.
            self._resume.set()
            for writer in tuple(self._writers | self._data_writers):
                writer.transport.abort()
            tasks = tuple(self._tasks)
            for task in tasks:
                task.cancel()
            await asyncio.gather(*tasks, return_exceptions=True)
            await asyncio.gather(*(server.wait_closed() for server in self._servers))
            self._writers.clear()
            self._clients.clear()
            self._upstreams.clear()
        finally:
            self._closed.set()


async def run(args):
    require_held_lock(FIXTURE_LOCK, 9)
    fixture = OwnedFixture(args.container_id, args.container_name, args.task_label, args.container_port)
    destination = fixture.inspect()
    proxy = FaultProxy(destination, args.lifetime_seconds)
    loop = asyncio.get_running_loop()
    for number in (signal.SIGINT, signal.SIGTERM):
        loop.add_signal_handler(number, lambda: asyncio.create_task(proxy.close()))
    try:
        # Fail before listening if the exact owned identity/binding has changed.
        if fixture.inspect() != destination:
            raise ValueError("fixture binding changed")
        print(json.dumps(await proxy.start()), flush=True)
        await proxy.wait_closed()
    finally:
        await proxy.close()
        for number in (signal.SIGINT, signal.SIGTERM):
            loop.remove_signal_handler(number)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--container-id", required=True)
    parser.add_argument("--container-name", required=True)
    parser.add_argument("--task-label", required=True)
    parser.add_argument("--container-port", required=True, type=int)
    parser.add_argument("--lifetime-seconds", type=int, default=MAX_LIFETIME_SECONDS)
    try:
        asyncio.run(run(parser.parse_args()))
    except (ValueError, OSError, subprocess.SubprocessError):
        parser.exit(2, "fault proxy refused: invalid ownership, lock or bounded configuration\n")


if __name__ == "__main__":
    main()
