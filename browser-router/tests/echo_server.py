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
        else:
            await socket.close(code=1008)
            return


async def main():
    async with serve(handle, "127.0.0.1", 18765):
        print("READY", flush=True)
        await asyncio.Future()


if __name__ == "__main__":
    asyncio.run(main())
