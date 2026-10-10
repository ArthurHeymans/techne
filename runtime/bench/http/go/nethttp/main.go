// The same answers as ../../techne/server.scm with net/http, Go's
// standard server (full request parsing). GOMAXPROCS sets the threads.
package main

import (
	"net/http"
	"os"
	"strconv"
)

func work(n int) int {
	acc := 0
	for i := 0; i < n; i++ {
		acc += len(strconv.Itoa(i * i))
	}
	return acc
}

func main() {
	hello := []byte("Hello, World!")
	http.HandleFunc("/", func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Server", "go")
		w.Header().Set("Content-Type", "text/plain")
		if r.URL.Path == "/work" {
			w.Write([]byte(strconv.Itoa(work(1000))))
		} else {
			w.Write(hello)
		}
	})
	if err := http.ListenAndServe("127.0.0.1:"+os.Args[1], nil); err != nil {
		panic(err)
	}
}
