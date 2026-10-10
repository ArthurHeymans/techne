// The same minimal server as ../../techne/server.scm: a goroutine per
// connection on raw net.Conn. GOMAXPROCS sets the threads.
package main

import (
	"bytes"
	"net"
	"os"
	"strconv"
)

var hello = []byte("HTTP/1.1 200 OK\r\nServer: go\r\nContent-Type: text/plain\r\nContent-Length: 13\r\n\r\nHello, World!")

func work(n int) int {
	acc := 0
	for i := 0; i < n; i++ {
		acc += len(strconv.Itoa(i * i))
	}
	return acc
}

func answer(req []byte, out []byte) []byte {
	if bytes.HasPrefix(req, []byte("GET /work ")) {
		body := strconv.Itoa(work(1000))
		out = append(out, "HTTP/1.1 200 OK\r\nServer: go\r\nContent-Type: text/plain\r\nContent-Length: "...)
		out = strconv.AppendInt(out, int64(len(body)), 10)
		out = append(out, "\r\n\r\n"...)
		return append(out, body...)
	}
	return append(out, hello...)
}

func handle(c net.Conn) {
	defer c.Close()
	chunk := make([]byte, 16*1024)
	var buf, out []byte
	for {
		n, err := c.Read(chunk)
		if n == 0 || err != nil {
			return
		}
		buf = append(buf, chunk[:n]...)
		start := 0
		for {
			end := bytes.Index(buf[start:], []byte("\r\n\r\n"))
			if end < 0 {
				break
			}
			out = answer(buf[start:], out)
			start += end + 4
		}
		buf = append(buf[:0], buf[start:]...)
		if len(out) > 0 {
			if _, err := c.Write(out); err != nil {
				return
			}
			out = out[:0]
		}
	}
}

func main() {
	l, err := net.Listen("tcp", "127.0.0.1:"+os.Args[1])
	if err != nil {
		panic(err)
	}
	for {
		c, err := l.Accept()
		if err != nil {
			panic(err)
		}
		go handle(c)
	}
}
