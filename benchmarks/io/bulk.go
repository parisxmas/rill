package main

import (
	"fmt"
	"net"
	"os"
	"strings"
)

func main() {
	port := os.Args[1]
	ln, err := net.Listen("tcp", "127.0.0.1:"+port)
	if err != nil {
		panic(err)
	}
	results := make(chan int)
	go func() {
		c, err := ln.Accept()
		if err != nil {
			panic(err)
		}
		buf := make([]byte, 64*1024)
		got := 0
		for {
			n, err := c.Read(buf)
			got += n
			if err != nil {
				break
			}
		}
		results <- got
	}()
	fd, err := net.Dial("tcp", "127.0.0.1:"+port)
	if err != nil {
		panic(err)
	}
	chunk := []byte(strings.Repeat("z", 65536))
	for k := 0; k < 1024; k++ {
		fd.Write(chunk)
	}
	fd.Close()
	fmt.Println(<-results)
}
