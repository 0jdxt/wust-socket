#!/usr/bin/env python

import asyncio
from websockets.asyncio.server import serve


async def echo(ws):
    async for message in ws:
        print("receieved message:", message)
        await ws.send(message)


async def main():
    print(f"listening on server localhost:8765")
    async with serve(echo, "localhost", 8765) as server:
        await server.serve_forever()

asyncio.run(main())
