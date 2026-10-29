"""Socket/tool boundary evidence only, not PG/S3 consequential recovery proof.

Run only inside the parent-routed fixture flock. All echo listeners are owned by
this test process, bounded by each test timeout and closed before completion.
"""

import asyncio
import copy
import errno
import fcntl
import importlib.util
import json
import os
import pathlib
import sys
import tempfile
import unittest

spec = importlib.util.spec_from_file_location(
    "deepref_fault_proxy", pathlib.Path(__file__).resolve().parents[1] / "fault_proxy.py"
)
module = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = module
spec.loader.exec_module(module)


def setUpModule():
    module.require_held_lock(module.FIXTURE_LOCK, 9)


class FaultProxyTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        self.writers = set()
        self.echo_tasks = set()
        self.clients = []
        self.fixture_ports = []
        self.upstream_received = asyncio.Queue()
        self.server = await asyncio.start_server(self.echo, "127.0.0.1", 0)
        port = self.server.sockets[0].getsockname()[1]
        self.proxy = module.FaultProxy(module.Destination(port))
        self.ports = await self.proxy.start()
        self.record_fixtures()

    def record_fixtures(self):
        ports = {"upstream": self.server.sockets[0].getsockname()[1],
                 "proxy": self.ports["proxy_port"], "control": self.ports["control_port"]}
        self.fixture_ports.extend(ports.values())
        if path := os.environ.get("DEEPREF_PROXY_TEST_EVIDENCE"):
            with open(path, "a", encoding="utf-8") as output:
                output.write(json.dumps({"test": self.id(), "pid": os.getpid(),
                                         "owned_loopback_ports": ports}) + "\n")

    async def asyncTearDown(self):
        await self.proxy.close()
        self.server.close()
        for writer in self.clients + list(self.writers):
            writer.transport.abort()
        tasks = tuple(self.echo_tasks)
        for task in tasks:
            task.cancel()
        await asyncio.gather(*tasks, return_exceptions=True)
        await self.server.wait_closed()
        for port in set(self.fixture_ports):
            with self.assertRaises(ConnectionRefusedError):
                await asyncio.open_connection("127.0.0.1", port)
        if path := os.environ.get("DEEPREF_PROXY_TEST_EVIDENCE"):
            with open(path, "a", encoding="utf-8") as output:
                output.write(json.dumps({"test": self.id(), "pid": os.getpid(),
                                         "closed_ports_verified": sorted(set(self.fixture_ports))}) + "\n")
        await asyncio.wait_for(self.proxy.wait_closed(), 1)

    async def echo(self, reader, writer):
        task = asyncio.current_task()
        self.echo_tasks.add(task)
        self.writers.add(writer)
        try:
            while data := await reader.read(65536):
                self.upstream_received.put_nowait(data)
                writer.write(data)
                await writer.drain()
        except (ConnectionError, OSError):
            pass
        finally:
            writer.close()
            self.writers.discard(writer)
            self.echo_tasks.discard(task)

    async def connect(self, key="proxy_port"):
        reader, writer = await asyncio.open_connection("127.0.0.1", self.ports[key])
        self.clients.append(writer)
        return reader, writer

    async def control(self, command, token=True):
        reader, writer = await self.connect("control_port")
        if token and isinstance(command, dict):
            command = {"token": self.ports["control_token"], **command}
        try:
            writer.write(json.dumps(command).encode() + b"\n")
            await writer.drain()
            return json.loads(await asyncio.wait_for(reader.readline(), 3))
        finally:
            writer.close()
            await writer.wait_closed()

    async def exchange(self, data=b"actual request"):
        reader, writer = await self.connect()
        writer.write(data)
        await writer.drain()
        self.assertEqual(await asyncio.wait_for(reader.readexactly(len(data)), 1), data)
        writer.close()
        await writer.wait_closed()

    async def test_stalled_response_preserves_actual_upstream_request_then_recovers(self):
        reader, writer = await self.connect()
        self.assertTrue((await self.control({"mode": "stall_response", "seconds": 10}))["ok"])
        writer.write(b"actual request")
        await writer.drain()
        self.assertEqual(await asyncio.wait_for(self.upstream_received.get(), 1), b"actual request")
        with self.assertRaises(TimeoutError):
            await asyncio.wait_for(reader.readexactly(14), 0.05)
        self.assertTrue((await self.control({"mode": "pass", "seconds": 1}))["ok"])
        self.assertEqual(await asyncio.wait_for(reader.readexactly(14), 1), b"actual request")

    async def test_half_close_preserves_the_upstream_response(self):
        async def after_eof(reader, writer):
            task = asyncio.current_task()
            self.echo_tasks.add(task)
            self.writers.add(writer)
            try:
                request = await reader.read()
                self.upstream_received.put_nowait(request)
                # A response generated after request EOF must survive. This also
                # makes cancelling on the first completed relay detectably wrong.
                await asyncio.sleep(0.02)
                writer.write(b"response after EOF")
                await writer.drain()
            finally:
                writer.close()
                self.writers.discard(writer)
                self.echo_tasks.discard(task)

        await self.proxy.close()
        self.server.close()
        await self.server.wait_closed()
        self.server = await asyncio.start_server(after_eof, "127.0.0.1", 0)
        self.proxy = module.FaultProxy(module.Destination(self.server.sockets[0].getsockname()[1]))
        self.ports = await self.proxy.start()
        self.record_fixtures()
        reader, writer = await self.connect()
        writer.write(b"request before EOF")
        await writer.drain()
        writer.write_eof()
        self.assertEqual(await asyncio.wait_for(reader.read(), 1), b"response after EOF")
        self.assertEqual(await asyncio.wait_for(self.upstream_received.get(), 1), b"request before EOF")

    async def test_drop_closes_existing_and_new_connections_then_expires(self):
        reader, writer = await self.connect()
        writer.write(b"ready")
        await writer.drain()
        self.assertEqual(await asyncio.wait_for(reader.readexactly(5), 1), b"ready")
        self.assertTrue((await self.control({"mode": "drop", "seconds": 1}))["ok"])
        self.assertEqual(await asyncio.wait_for(reader.read(1), 1), b"")
        rejected, _ = await self.connect()
        self.assertEqual(await asyncio.wait_for(rejected.read(1), 1), b"")
        await asyncio.sleep(1.05)
        await self.exchange(b"recovered")

    async def test_stall_automatically_releases_without_a_recovery_command(self):
        reader, writer = await self.connect()
        self.assertTrue((await self.control({"mode": "stall_response", "seconds": 1}))["ok"])
        writer.write(b"ambiguous")
        await writer.drain()
        self.assertEqual(await asyncio.wait_for(self.upstream_received.get(), 1), b"ambiguous")
        with self.assertRaises(TimeoutError):
            await asyncio.wait_for(reader.read(1), 0.05)
        self.assertEqual(await asyncio.wait_for(reader.readexactly(9), 2), b"ambiguous")

    async def test_invalid_control_is_bounded_and_never_reflects_input(self):
        for command in [
            {"mode": "drop", "seconds": 0},
            {"mode": "drop", "seconds": 31},
            {"mode": "drop", "seconds": True},
            {"mode": {}, "seconds": 1},
            {"mode": "pass", "seconds": 1, "upstream": "PRIVATE-PAYLOAD"},
            {"mode": "drop", "seconds": 1, "token": "wrong"},
            ["drop", 1],
        ]:
            self.assertEqual(await self.control(command), {
                "ok": False, "error": "invalid bounded fault command"
            })
        self.assertFalse((await self.control({"mode": "drop", "seconds": 1}, token=False))["ok"])
        reader, writer = await self.connect("control_port")
        writer.write(b"PRIVATE-PAYLOAD" * module.CONTROL_BYTES + b"\n")
        await writer.drain()
        response = await asyncio.wait_for(reader.readline(), 3)
        self.assertNotIn(b"PRIVATE-PAYLOAD", response)
        self.assertFalse(json.loads(response)["ok"])
        # A bounded but deeply nested JSON value must not kill the proxy.
        reader, writer = await self.connect("control_port")
        writer.write(b"[" * 1500 + b"]" * 1500 + b"\n")
        await writer.drain()
        self.assertFalse(json.loads(await asyncio.wait_for(reader.readline(), 3))["ok"])
        await self.exchange()

    async def test_data_and_control_admission_caps_refuse_excess_connections(self):
        for _ in range(16):
            reader, writer = await self.connect()
            writer.write(b"ready")
            await writer.drain()
            self.assertEqual(await asyncio.wait_for(reader.readexactly(5), 1), b"ready")
        reader, _ = await self.connect()
        self.assertEqual(await asyncio.wait_for(reader.read(1), 1), b"")
        slow = []
        for _ in range(4):
            reader, writer = await self.connect("control_port")
            writer.write(b"{")
            await writer.drain()
            slow.append(reader)
        reader, _ = await self.connect("control_port")
        self.assertEqual(await asyncio.wait_for(reader.read(1), 1), b"")
        for reader in slow:
            self.assertFalse(json.loads(await asyncio.wait_for(reader.readline(), 3))["ok"])
        # Slow controls expire and the control interface remains usable.
        self.assertTrue((await self.control({"mode": "pass", "seconds": 1}))["ok"])

    async def test_close_interrupts_active_stall_and_all_listeners(self):
        reader, writer = await self.connect()
        await self.control({"mode": "stall_response", "seconds": 30})
        writer.write(b"pending")
        await writer.drain()
        self.assertEqual(await asyncio.wait_for(self.upstream_received.get(), 1), b"pending")
        slow, slow_writer = await self.connect("control_port")
        slow_writer.write(b"{")
        await slow_writer.drain()
        await asyncio.wait_for(self.proxy.close(), 1)
        await asyncio.wait_for(self.proxy.wait_closed(), 1)
        self.assertEqual(await asyncio.wait_for(reader.read(1), 1), b"")
        # Aborting unread partial control input may close with TCP RST.
        # Require closure within the same bound; bytes/timeout/other errors fail.
        try:
            data = await asyncio.wait_for(slow.read(1), 1)
        except ConnectionResetError as error:
            self.assertEqual(error.errno, errno.ECONNRESET)
        else:
            self.assertEqual(data, b"")
        for key in ("proxy_port", "control_port"):
            with self.assertRaises(ConnectionRefusedError):
                await asyncio.open_connection("127.0.0.1", self.ports[key])
        # Closing twice is safe and does not deadlock.
        await asyncio.wait_for(self.proxy.close(), 1)

    async def test_process_lifetime_closes_live_traffic_without_caller_cleanup(self):
        await self.proxy.close()
        port = self.server.sockets[0].getsockname()[1]
        self.proxy = module.FaultProxy(module.Destination(port), lifetime_seconds=1)
        self.ports = await self.proxy.start()
        self.record_fixtures()
        reader, writer = await self.connect()
        writer.write(b"before expiry")
        await writer.drain()
        self.assertEqual(await asyncio.wait_for(reader.readexactly(13), 1), b"before expiry")
        await asyncio.wait_for(self.proxy.wait_closed(), 2)
        self.assertEqual(await asyncio.wait_for(reader.read(1), 1), b"")
        for key in ("proxy_port", "control_port"):
            with self.assertRaises(ConnectionRefusedError):
                await asyncio.open_connection("127.0.0.1", self.ports[key])

    def test_only_literal_loopback_and_bounded_ports_lifetimes_are_accepted(self):
        for host in ["localhost", "provider.example", "127.1.2.3", "localhost.example", "0.0.0.0"]:
            with self.assertRaises(ValueError):
                module.Destination(5432, host)
        for port in [0, -1, 65536, True, 1.5]:
            with self.assertRaises(ValueError):
                module.Destination(port)
        for seconds in [0, 301, True, 1.5]:
            with self.assertRaises(ValueError):
                module.FaultProxy(module.Destination(5432), lifetime_seconds=seconds)


class FixtureBoundaryTests(unittest.TestCase):
    """Catalog parsing/actual flock semantics; not Docker provisioning proof."""

    def setUp(self):
        self.fixture = module.OwnedFixture(
            "a" * 64, "deepref-ops-fault-pg-12345678", "operations-fault-12345678", 5432
        )
        self.catalog = {
            "id": "a" * 64, "name": "/deepref-ops-fault-pg-12345678",
            "task": "operations-fault-12345678", "running": True, "paused": False,
            "binding": [{"HostIp": "127.0.0.1", "HostPort": "54321"}],
        }

    def test_exact_identity_and_one_loopback_binding_are_required(self):
        self.assertEqual(self.fixture.destination(self.catalog), module.Destination(54321))
        changes = [
            ("id", "b" * 64), ("name", "/another-fixture"), ("task", "another-task"),
            ("running", False), ("paused", True), ("binding", None), ("binding", []),
            ("binding", [{"HostIp": "0.0.0.0", "HostPort": "54321"}]),
            ("binding", [{"HostIp": "::1", "HostPort": "54321"}]),
            ("binding", [{"HostIp": "127.0.0.1", "HostPort": "0"}]),
            ("binding", [{"HostIp": "127.0.0.1", "HostPort": True}]),
            ("binding", self.catalog["binding"] * 2),
        ]
        for key, value in changes:
            changed = copy.deepcopy(self.catalog)
            changed[key] = value
            with self.subTest(key=key, value=value), self.assertRaises(ValueError):
                self.fixture.destination(changed)

    def test_missing_unheld_wrong_inode_or_other_owners_lock_is_refused(self):
        with tempfile.TemporaryDirectory(prefix="deepref-proxy-lock-test-") as directory:
            path = pathlib.Path(directory) / "fixture.lock"
            other_path = pathlib.Path(directory) / "other.lock"
            with open(path, "w+b") as owner, open(path, "rb") as stranger, open(other_path, "w+b") as other:
                with self.assertRaises(ValueError):
                    module.require_held_lock(path, owner.fileno())
                fcntl.flock(owner.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
                module.require_held_lock(path, owner.fileno())
                with self.assertRaises(BlockingIOError):
                    module.require_held_lock(path, stranger.fileno())
                with self.assertRaises(ValueError):
                    module.require_held_lock(path, other.fileno())
                with self.assertRaises(OSError):
                    module.require_held_lock(path, -1)
                # The verifier must not release the actual owner's flock.
                with self.assertRaises(BlockingIOError):
                    fcntl.flock(stranger.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)


if __name__ == "__main__":
    unittest.main()
