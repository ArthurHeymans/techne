// The same minimal server as ../techne/server.scm on node:net. One event
// loop per process; run several with reusePort for more cores.
const net = require("node:net");

const hello = Buffer.from("HTTP/1.1 200 OK\r\nServer: node\r\nContent-Type: text/plain\r\nContent-Length: 13\r\n\r\nHello, World!");
const GET_WORK = Buffer.from("GET /work ");
const END = Buffer.from("\r\n\r\n");

function work(n) {
  let acc = 0;
  for (let i = 0; i < n; i++) acc += String(i * i).length;
  return acc;
}

function answer(req) {
  if (req.subarray(0, GET_WORK.length).equals(GET_WORK)) {
    const body = String(work(1000));
    return Buffer.from(`HTTP/1.1 200 OK\r\nServer: node\r\nContent-Type: text/plain\r\nContent-Length: ${body.length}\r\n\r\n${body}`);
  }
  return hello;
}

net
  .createServer({ noDelay: true }, (socket) => {
    let buf = Buffer.alloc(0);
    socket.on("data", (chunk) => {
      buf = buf.length ? Buffer.concat([buf, chunk]) : chunk;
      const out = [];
      let start = 0, end;
      while ((end = buf.indexOf(END, start)) >= 0) {
        out.push(answer(buf.subarray(start)));
        start = end + 4;
      }
      buf = buf.subarray(start);
      if (out.length === 1) socket.write(out[0]);
      else if (out.length) socket.write(Buffer.concat(out));
    });
    socket.on("error", () => {});
  })
  .listen({ host: "127.0.0.1", port: Number(process.argv[2]), reusePort: true });
