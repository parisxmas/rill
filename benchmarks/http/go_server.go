// The same one-route server in Go, for comparison with rill_server.rill.
//
// Keep-alive stays enabled here and the *client* decides whether to reuse the
// connection, which is how the Rill server is driven too. Turning it off with
// SetKeepAlivesEnabled(false) instead would be unfair to Go: that puts the
// server in shutdown mode, and it then serves ~1.6k req/s rather than the
// ~16k it manages when the client asks for the close.
package main

import (
	"flag"
	"net/http"
)

func main() {
	port := flag.String("port", "8101", "port")
	flag.Parse()
	mux := http.NewServeMux()
	mux.HandleFunc("/", func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/plain")
		w.Write([]byte("Hello, World!"))
	})
	http.ListenAndServe(":"+*port, mux)
}
