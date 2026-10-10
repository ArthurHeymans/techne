"""The same minimal server as ../techne/server.scm on an asyncio Protocol.

One event loop per process; run several with SO_REUSEPORT for more cores.
"""

import asyncio
import socket
import sys

HELLO = b"HTTP/1.1 200 OK\r\nServer: python\r\nContent-Type: text/plain\r\nContent-Length: 13\r\n\r\nHello, World!"


def work(n):
    return sum(len(str(i * i)) for i in range(n))


def answer(req):
    if req.startswith(b"GET /work "):
        body = str(work(1000)).encode()
        return (
            b"HTTP/1.1 200 OK\r\nServer: python\r\nContent-Type: text/plain\r\nContent-Length: %d\r\n\r\n%s"
            % (len(body), body)
        )
    return HELLO


class Http(asyncio.Protocol):
    def connection_made(self, transport):
        self.transport: asyncio.Transport = transport  # type: ignore[assignment]
        self.buf = b""
        transport.get_extra_info("socket").setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)

    def data_received(self, data):
        buf = self.buf + data if self.buf else data
        out, start = [], 0
        while (end := buf.find(b"\r\n\r\n", start)) >= 0:
            out.append(answer(buf[start:]))
            start = end + 4
        self.buf = buf[start:]
        if out:
            self.transport.write(b"".join(out))


async def main(port):
    server = await asyncio.get_running_loop().create_server(Http, "127.0.0.1", port, reuse_port=True, backlog=4096)
    await server.serve_forever()


asyncio.run(main(int(sys.argv[1])))
