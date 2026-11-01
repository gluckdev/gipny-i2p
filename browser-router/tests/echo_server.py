"""CI fixture only: no TCP forwarding, authentication, or access to live I2P."""
import asyncio
from urllib.parse import parse_qs, urlsplit
from websockets.asyncio.server import serve


async def handle(socket):
    request = urlsplit(socket.request.path)
    targets = parse_qs(request.query).get("target", [])
    if request.path != "/stream" or len(targets) != 1:
        await socket.close(code=1008)
        return
    target = targets[0]
    received = 0
    checksum = 0
    async for data in socket:
        if not isinstance(data, bytes):
            await socket.close(code=1003)
            return
        if target == "127.0.0.1:10101":
            await socket.send(data[:3])
            await socket.send(data[3:])
        elif target == "127.0.0.1:10102":
            await socket.send("text is not a TCP byte stream")
        elif target == "127.0.0.1:10103":
            await socket.send(bytes(65537))
        elif target == "127.0.0.1:10105":
            socket.transport.abort()
            return
        elif target == "127.0.0.1:10106":
            await socket.send(b"")
            await socket.send(data)
            await socket.close()
            return
        elif target == "127.0.0.1:10107":
            for _ in range(32):
                await socket.send(bytes(16384))
        elif target == "127.0.0.1:10108":
            if data == b"done":
                await socket.send(received.to_bytes(8, "big") + checksum.to_bytes(8, "big"))
            else:
                received += len(data)
                checksum += sum(data)
        else:
            await socket.close(code=1008)
            return


async def main():
    http = await asyncio.start_server(snapshot_http, "127.0.0.1", 18766)
    async with http, serve(handle, "127.0.0.1", 18765):
        print("READY", flush=True)
        await asyncio.Future()


async def snapshot_http(reader, writer):
    try:
        request = await asyncio.wait_for(reader.readuntil(b"\r\n\r\n"), 3)
        path = request.split(b" ")[1]
        common = b"Access-Control-Allow-Origin: *\r\nConnection: close\r\n"
        if path == b"/oversize":
            writer.write(b"HTTP/1.1 200 OK\r\n" + common + b"Content-Length: 4194305\r\n\r\n")
        elif path == b"/stream-oversize":
            writer.write(b"HTTP/1.1 200 OK\r\n" + common + b"\r\n")
            for _ in range(65):
                writer.write(bytes(65536))
                await writer.drain()
        elif path == b"/redirect":
            writer.write(b"HTTP/1.1 302 Found\r\n" + common + b"Location: /bad-gzip\r\nContent-Length: 0\r\n\r\n")
        elif path == b"/bad-gzip":
            writer.write(b"HTTP/1.1 200 OK\r\n" + common + b"Content-Length: 3\r\n\r\nbad")
        elif path == b"/truncated":
            writer.write(b"HTTP/1.1 200 OK\r\n" + common + b"Content-Length: 100\r\n\r\nbad")
        else:
            writer.write(b"HTTP/1.1 404 Not Found\r\n" + common + b"Content-Length: 0\r\n\r\n")
        await writer.drain()
    except (ConnectionError, asyncio.IncompleteReadError, asyncio.LimitOverrunError, TimeoutError):
        pass
    finally:
        writer.close()
        try:
            await writer.wait_closed()
        except ConnectionError:
            pass


if __name__ == "__main__":
    asyncio.run(main())
