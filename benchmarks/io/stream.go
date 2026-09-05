package main

import (
	"bufio"
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
	w := bufio.NewWriterSize(fd, 64*1024)
	msg := []byte(strings.Repeat("m", 32))
	for k := 0; k < 1000000; k++ {
		w.Write(msg)
	}
	w.Flush()
	fd.Close()
	fmt.Println(<-results)
}
