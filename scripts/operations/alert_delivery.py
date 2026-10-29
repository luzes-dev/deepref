#!/usr/bin/env python3
"""Bounded process-owned loopback alert test, never a production notification.

The CLI requires the inherited fixture flock on fd9. It accepts a catalog rule
and a new evidence path, never a notification URL, socket port or credential.
"""

import argparse
import asyncio
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import secrets

from alert_protocol import (
    Binding, MAX_FRAME, OneShotReceiver, REQUEST_DEADLINE_SECONDS,
    new_request, validate_request, verify_acknowledgement, encode_frame,
)
from fault_proxy import FIXTURE_LOCK, require_held_lock
from metrics_catalog import read_json_bounded, validate_catalog

MAX_CLIENTS = 4
PROCESS_SECONDS = 20
CLOSE_SECONDS = 1


def utc_now():
    return datetime.now(timezone.utc)


class LocalReceiver:
    """One test binding, one private listener, bounded clients and cleanup."""

    def __init__(self, binding):
        self.binding = binding
        self._nonce = secrets.token_hex(16)
        self._token = secrets.token_hex(32)
        self._protocol = OneShotReceiver(binding, nonce=self._nonce, token=self._token)
        self._server = None
        self._port = None
        self._tasks = set()
        self._writers = set()
        self._closing = False
        self._closed = asyncio.Event()
        self.refused = 0
        self.deadlines = 0
        self.failures = []

    async def start(self):
        if self._server is not None or self._closing:
            raise RuntimeError("local test receiver cannot restart")
        self._server = await asyncio.start_server(
            self._accept, "127.0.0.1", 0, limit=MAX_FRAME, backlog=MAX_CLIENTS)
        self._port = self._server.sockets[0].getsockname()[1]
        return self

    @property
    def port(self):
        if self._port is None:
            raise RuntimeError("local test receiver not started")
        return self._port

    @property
    def pending_tasks(self):
        return len(self._tasks)

    def request(self):
        return new_request(self.binding, nonce=self._nonce, token=self._token,
                           issued_at=utc_now().isoformat().replace("+00:00", "Z"))

    def _accept(self, reader, writer):
        if self._closing or len(self._tasks) >= MAX_CLIENTS:
            self.refused += 1
            writer.transport.abort()
            return
        writer.transport.set_write_buffer_limits(high=MAX_FRAME, low=MAX_FRAME // 4)
        self._writers.add(writer)
        task = asyncio.create_task(self._serve(reader, writer))
        self._tasks.add(task)

        def finished(done):
            self._tasks.discard(done)
            self._writers.discard(writer)
            if not done.cancelled() and done.exception() is not None:
                # Diagnostic type only; never protocol frames, token or payload.
                self.failures.append(type(done.exception()).__name__)

        task.add_done_callback(finished)

    async def response(self, frame):
        return self._protocol.accept(frame, now=utc_now())

    async def _serve(self, reader, writer):
        try:
            async with asyncio.timeout(REQUEST_DEADLINE_SECONDS):
                response = await self.response(await reader.readline())
                writer.write(response)
                await writer.drain()
        except ValueError:
            self.refused += 1
        except TimeoutError:
            self.deadlines += 1
        finally:
            try:
                if not self._closing:
                    writer.close()
                    async with asyncio.timeout(CLOSE_SECONDS):
                        await writer.wait_closed()
            finally:
                writer.transport.abort()

    async def exchange(self, row):
        """Sender destination is this very receiver's process-owned listener."""
        loop = asyncio.get_running_loop()
        started = loop.time()
        request = validate_request(encode_frame(row), binding=self.binding,
                                   nonce=self._nonce, token=self._token, now=utc_now())
        writer = None
        try:
            async with asyncio.timeout(REQUEST_DEADLINE_SECONDS):
                reader, writer = await asyncio.open_connection("127.0.0.1", self.port, limit=MAX_FRAME)
                writer.write(encode_frame(row))
                await writer.drain()
                response = await reader.readline()
                receipt = verify_acknowledgement(response, request=request,
                                                 now=utc_now(), elapsed_seconds=loop.time() - started)
                # This receipt is emitted only after an actual matching wire ACK.
                receipt.update(wire_delivery_verified=True, local_fixture_delivery=True)
                return receipt
        finally:
            if writer is not None:
                writer.close()
                try:
                    async with asyncio.timeout(CLOSE_SECONDS):
                        await writer.wait_closed()
                finally:
                    writer.transport.abort()

    async def close(self):
        if self._closing:
            await self._closed.wait()
            return
        self._closing = True
        try:
            if self._server is not None:
                self._server.close()
            # Python3.14 wait_closed waits for accepted clients too. Close owned
            # clients/tasks before waiting; listener closure alone is insufficient.
            for writer in list(self._writers):
                writer.transport.abort()
            pending = list(self._tasks)
            for task in pending:
                task.cancel()
            async with asyncio.timeout(CLOSE_SECONDS):
                outcomes = await asyncio.gather(*pending, return_exceptions=True)
                if any(isinstance(value, BaseException) and not isinstance(value, asyncio.CancelledError) for value in outcomes):
                    raise RuntimeError("local test client cleanup failed")
                if self._server is not None:
                    await self._server.wait_closed()
            if self._tasks or self._writers or self.failures:
                raise RuntimeError("local test has unfinished or failed clients")
        finally:
            self._closed.set()


async def require_closed(port):
    """Independent exact-port check, never a global socket inventory."""
    try:
        async with asyncio.timeout(CLOSE_SECONDS):
            _, writer = await asyncio.open_connection("127.0.0.1", port)
    except ConnectionRefusedError:
        return
    writer.transport.abort()
    raise RuntimeError("owned local alert listener still accepts connections")


async def local_delivery(binding, report_start):
    receiver = await LocalReceiver(binding).start()
    receipt = None
    try:
        report_start(receiver.port)
        async with asyncio.timeout(PROCESS_SECONDS):
            receipt = await receiver.exchange(receiver.request())
    finally:
        await receiver.close()
        await require_closed(receiver.port)
    receipt.update(owned_listener_closed=True, owned_port=receiver.port,
                   pending_tasks=receiver.pending_tasks)
    return receipt


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--catalog", type=Path, required=True)
    parser.add_argument("--repository", type=Path, required=True)
    parser.add_argument("--rule", required=True)
    parser.add_argument("--output", type=Path, required=True, help="new local fixture JSONL evidence file")
    args = parser.parse_args()
    try:
        require_held_lock(FIXTURE_LOCK, 9)
        catalog = read_json_bounded(args.catalog)
        validate_catalog(catalog, args.repository)
        rule = next((row for row in catalog["rules"] if row["id"] == args.rule), None)
        if rule is None:
            raise ValueError("unknown catalog rule")
        digest = hashlib.sha256(json.dumps(
            catalog, sort_keys=True, separators=(",", ":"), allow_nan=False
        ).encode("utf-8")).hexdigest()
        binding = Binding.from_validated_rule(rule, digest)
        # Reserve before opening a listener, with no overwrite of any evidence.
        with args.output.open("x", encoding="utf-8") as output:
            def report_start(port):
                json.dump({"phase": "listener_started", "owned_port": port,
                           "production_accepted": False}, output)
                output.write("\n")
                output.flush()

            try:
                receipt = asyncio.run(local_delivery(binding, report_start))
            except (ValueError, OSError, RuntimeError, TimeoutError) as error:
                json.dump({"local_fixture_delivery": False, "production_accepted": False,
                           "failure_type": type(error).__name__, "cleanup_proof": False}, output)
                output.write("\n")
                raise
            json.dump({"phase": "complete", **receipt}, output)
            output.write("\n")
    except (ValueError, OSError, RuntimeError, TimeoutError):
        parser.exit(2, "local alert test failed or lacks its owned fixture lock; inspect local evidence\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
