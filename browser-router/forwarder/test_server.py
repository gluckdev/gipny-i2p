import asyncio
from pathlib import Path
import tempfile
import time
import unittest
from urllib.parse import quote

from websockets.asyncio.client import connect
from websockets.exceptions import ConnectionClosed, InvalidStatus
from server import Forwarder, load_targets, PROTOCOL_PREFIX

TOKEN = "a" * 43
ORIGIN = "http://127.0.0.1:18769"


class ForwarderTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        self.dials = 0

        async def echo(reader, writer):
            self.dials += 1
            try:
                while chunk := await reader.read(65536):
                    writer.write(chunk)
                    await writer.drain()
            finally:
                writer.close()
                await writer.wait_closed()

        self.tcp = await asyncio.start_server(echo, "127.0.0.1", 0)
        self.target = f"127.0.0.1:{self.tcp.sockets[0].getsockname()[1]}"
        self.policy = Forwarder(TOKEN, {"placeholder"}, {ORIGIN}, {self.target: time.time() + 60})
        self.ws = await self.policy.listen("127.0.0.1", 0)
        self.host = f"127.0.0.1:{self.ws.sockets[0].getsockname()[1]}"
        self.policy.hosts = frozenset({self.host})
        self.url = f"ws://{self.host}/stream?target={quote(self.target)}"

    async def asyncTearDown(self):
        self.ws.close()
        await self.ws.wait_closed()
        self.tcp.close()
        await self.tcp.wait_closed()
        if self.policy.watchers:
            await asyncio.gather(*self.policy.watchers, return_exceptions=True)

    def client(self, url=None, *, origin=ORIGIN, token=TOKEN, **kwargs):
        return connect(url or self.url, origin=origin, subprotocols=[PROTOCOL_PREFIX + token], proxy=None, **kwargs)

    async def test_rejects_before_dialing_tcp(self):
        attempts = [
            self.client(origin="https://attacker.example"), self.client(origin=None),
            self.client(token="b" * 43),
            self.client(additional_headers={"Host": "attacker.example"}),
            self.client(additional_headers={"Origin": ORIGIN}),
            self.client(additional_headers={"Sec-WebSocket-Key": "invalid"}),
            self.client(self.url + "&target=" + quote(self.target)),
            self.client(self.url + "&extra=1"),
            self.client(f"ws://{self.host}/stream?target=127.0.0.1:1"),
            self.client(f"ws://{self.host}/stream?target=localhost:1234"),
        ]
        for attempt in attempts:
            with self.assertRaises(InvalidStatus):
                async with attempt:
                    self.fail("rejected request accepted")
        self.assertEqual(self.dials, 0)
        self.assertFalse(self.policy.leases)

    async def test_binary_stream_order_and_tcp_connected_before_upgrade(self):
        async with self.client() as client:
            self.assertEqual(self.dials, 1)
            payload = bytes(range(256)) * 400
            for start in range(0, len(payload), 65536):
                await client.send(payload[start:start + 65536])
            result = b""
            while len(result) < len(payload):
                result += await client.recv()
            self.assertEqual(result, payload)

    async def test_admission_and_expiry(self):
        self.policy.max_connections = 1
        async with self.client():
            with self.assertRaises(InvalidStatus):
                async with self.client():
                    self.fail("connection limit bypass")
            self.assertEqual(self.dials, 1)
        self.policy.targets[self.target] = time.time() - 1
        with self.assertRaises(InvalidStatus):
            async with self.client():
                self.fail("expired target accepted")

    async def test_text_is_rejected(self):
        async with self.client() as client:
            await client.send("not bytes")
            with self.assertRaises(ConnectionClosed) as result:
                await client.recv()
            self.assertEqual(result.exception.rcvd.code, 1003)

    async def test_private_targets_need_explicit_fixture_exception(self):
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / "candidates.tsv"
            path.write_text(f"{self.target}\tri\tpeer\tfixture\t{int(time.time())}\n"
                f"10.0.0.1:1234\tri\tpeer\tnetdb\t{int(time.time())}\n")
            self.assertEqual(load_targets(path), {})
            self.assertEqual(set(load_targets(path, True)), {self.target})


if __name__ == "__main__":
    unittest.main()
