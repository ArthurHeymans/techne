// The same answers as ../techne/server.scm with node:http (full request
// parsing). One event loop per process; run several with reusePort.
const http = require("node:http");

function work(n) {
  let acc = 0;
  for (let i = 0; i < n; i++) acc += String(i * i).length;
  return acc;
}

const hello = Buffer.from("Hello, World!");
http
  .createServer({ noDelay: true }, (req, res) => {
    const body = req.url === "/work" ? String(work(1000)) : hello;
    res.writeHead(200, { Server: "node", "Content-Type": "text/plain", "Content-Length": body.length });
    res.end(body);
  })
  .listen({ host: "127.0.0.1", port: Number(process.argv[2]), reusePort: true });
