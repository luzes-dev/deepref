"""Real loopback tests: run only inside an explicitly routed socket batch.

This file refuses a missing inherited global fixture lock; it never silently
skips or creates a lock. Pure tests live in test_alert_protocol.py.
"""

import asyncio
from datetime import timedelta
import errno
import json
import os
from pathlib import Path
import sys
import unittest

REPOSITORY = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(REPOSITORY / "scripts/operations"))
from alert_delivery import LocalReceiver, require_closed, utc_now
from alert_protocol import Binding, decode_frame, encode_frame
from fault_proxy import FIXTURE_LOCK, require_held_lock


class ChangedAckReceiver(LocalReceiver):
    def __init__(self, binding, change):
        super().__init__(binding)
        self.change = change

    async def response(self, frame):
        row = decode_frame(await super().response(frame))
        self.change(row)
        return encode_frame(row)


class NoAckReceiver(LocalReceiver):
    async def response(self, frame):
        await asyncio.Event().wait()


class ObservedReceiver(LocalReceiver):
    def __init__(self, binding):
        super().__init__(binding)
        self.accepted = asyncio.Event()

    def _accept(self, reader, writer):
        super()._accept(reader, writer)
        self.accepted.set()


class AlertWireTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        require_held_lock(FIXTURE_LOCK, 9)
        self.binding = Binding.from_validated_rule(
            {"id": "queue-age", "owner_role": "worker-owner", "delivery_test_id": "local-queue-age-v1"}, "b" * 64)
        self.receivers = []

    async def start(self, receiver=None):
        receiver = await (receiver or LocalReceiver(self.binding)).start()
        self.receivers.append(receiver)
        # The outer runner creates this exact owned manifest after acquisition.
        manifest = Path(os.environ["DEEPREF_ALERT_PORT_MANIFEST"])
        with manifest.open("a", encoding="utf-8") as out:
            out.write(json.dumps({"phase": "listener_started", "case": self.id(), "port": receiver.port}) + "\n")
        return receiver

    async def asyncTearDown(self):
        errors = []
        for receiver in self.receivers:
            try:
                await receiver.close()
                await require_closed(receiver.port)
                self.assertEqual(receiver.pending_tasks, 0)
                with Path(os.environ["DEEPREF_ALERT_PORT_MANIFEST"]).open("a", encoding="utf-8") as out:
                    out.write(json.dumps({"phase": "closed", "case": self.id(), "port": receiver.port,
                                          "pending_tasks": receiver.pending_tasks, "refused": receiver.refused,
                                          "deadlines": receiver.deadlines, "failed_clients": receiver.failures}) + "\n")
            except BaseException as error:
                errors.append(error)
        if errors:
            raise BaseExceptionGroup("owned alert receiver cleanup failed", errors)

    async def test_real_matching_ack_is_local_only_and_listener_closes(self):
        receiver = await self.start()
        result = await receiver.exchange(receiver.request())
        self.assertTrue(result["wire_delivery_verified"])
        self.assertTrue(result["local_fixture_delivery"])
        self.assertFalse(result["production_accepted"])
        self.assertFalse(result["backend_ingestion_verified"])
        self.assertFalse(result["human_notification_verified"])
        self.assertLessEqual(result["elapsed_seconds"], 2)

    async def test_wrong_route_owner_catalog_nonce_and_digest_ack_are_refused(self):
        for key, value in {"route": "local-api-owner-v1", "owner_role": "api-owner",
                           "catalog_sha256": "c" * 64, "nonce": "d" * 32,
                           "request_sha256": "e" * 64}.items():
            with self.subTest(field=key):
                receiver = await self.start(ChangedAckReceiver(self.binding, lambda row, k=key, v=value: row.update({k: v})))
                with self.assertRaisesRegex(ValueError, "binding mismatch"):
                    await receiver.exchange(receiver.request())
                await receiver.close()

    async def test_future_and_backdated_ack_are_refused(self):
        for seconds in (-10, 10):
            with self.subTest(seconds=seconds):
                value = (utc_now() + timedelta(seconds=seconds)).isoformat().replace("+00:00", "Z")
                receiver = await self.start(ChangedAckReceiver(self.binding, lambda row, v=value: row.update(accepted_at=v)))
                with self.assertRaisesRegex(ValueError, "stale or future"):
                    await receiver.exchange(receiver.request())
                await receiver.close()

    async def test_missing_ack_hits_actual_whole_request_timeout(self):
        receiver = await self.start(NoAckReceiver(self.binding))
        started = asyncio.get_running_loop().time()
        with self.assertRaises(TimeoutError):
            await receiver.exchange(receiver.request())
        elapsed = asyncio.get_running_loop().time() - started
        self.assertGreaterEqual(elapsed, 1.9)
        self.assertLess(elapsed, 3)

    async def test_duplicate_nonce_is_refused_over_wire(self):
        receiver = await self.start()
        row = receiver.request()
        await receiver.exchange(row)
        with self.assertRaises(ValueError):
            await receiver.exchange(row)
        self.assertEqual(receiver.refused, 1)

    async def test_wrong_request_route_does_not_consume_valid_delivery(self):
        receiver = await self.start()
        wrong = {**receiver.request(), "route": "local-api-owner-v1"}
        async with asyncio.timeout(2):
            reader, writer = await asyncio.open_connection("127.0.0.1", receiver.port)
            try:
                writer.write(encode_frame(wrong))
                await writer.drain()
                self.assertEqual(await reader.read(1), b"")
            finally:
                writer.close()
                await writer.wait_closed()
        self.assertTrue((await receiver.exchange(receiver.request()))["wire_delivery_verified"])
        self.assertEqual(receiver.refused, 1)

    async def test_partial_request_shutdown_has_no_task_or_listener_escape(self):
        receiver = await self.start(ObservedReceiver(self.binding))
        reader, writer = await asyncio.open_connection("127.0.0.1", receiver.port)
        try:
            writer.write(b'{"kind":')
            await writer.drain()
            async with asyncio.timeout(1):
                await receiver.accepted.wait()
                await receiver.close()
                try:
                    self.assertEqual(await reader.read(1), b"")
                except ConnectionResetError as error:
                    # This exact peer is intentionally aborted during shutdown.
                    # Normal ACK, graceful reply and timeout assertions stay strict.
                    self.assertEqual(error.errno, errno.ECONNRESET)
            self.assertEqual(receiver.pending_tasks, 0)
            await require_closed(receiver.port)
        finally:
            writer.transport.abort()

    async def test_four_client_cap_refuses_fifth_and_shutdown_drains_all(self):
        receiver = await self.start(ObservedReceiver(self.binding))
        writers = []
        try:
            for _ in range(5):
                receiver.accepted.clear()
                reader, writer = await asyncio.open_connection("127.0.0.1", receiver.port)
                writers.append(writer)
                async with asyncio.timeout(1):
                    await receiver.accepted.wait()
            self.assertEqual(receiver.pending_tasks, 4)
            self.assertEqual(receiver.refused, 1)
            async with asyncio.timeout(1):
                try:
                    self.assertEqual(await reader.read(1), b"")
                except ConnectionResetError as error:
                    # Fifth connection is explicitly rejected with transport.abort.
                    self.assertEqual(error.errno, errno.ECONNRESET)
            await receiver.close()
            self.assertEqual(receiver.pending_tasks, 0)
        finally:
            for writer in writers:
                writer.transport.abort()

    async def test_oversized_wire_frame_is_refused_without_consuming_nonce(self):
        receiver = await self.start()
        async with asyncio.timeout(2):
            reader, writer = await asyncio.open_connection("127.0.0.1", receiver.port)
            try:
                writer.write(b"x" * 4096 + b"\n")
                await writer.drain()
                self.assertEqual(await reader.read(1), b"")
            finally:
                writer.close()
                await writer.wait_closed()
        self.assertEqual(receiver.refused, 1)
        self.assertTrue((await receiver.exchange(receiver.request()))["wire_delivery_verified"])


if __name__ == "__main__":
    unittest.main()
