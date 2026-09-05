package main

import (
	"fmt"
	"io"
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
	go func() {
		for {
			c, err := ln.Accept()
			if err != nil {
				return
			}
			go func(c net.Conn) {
				buf := make([]byte, 64*1024)
				for {
					n, err := c.Read(buf)
					if err != nil {
						c.Close()
						return
					}
					c.Write(buf[:n])
				}
			}(c)
		}
	}()
	fd, err := net.Dial("tcp", "127.0.0.1:"+port)
	if err != nil {
		panic(err)
	}
	msg := []byte(strings.Repeat("x", 64))
	reply := make([]byte, 64)
	acc := 0
	for k := 0; k < 100000; k++ {
		fd.Write(msg)
		n, _ := io.ReadFull(fd, reply)
		acc += n
	}
	fmt.Println(acc)
	fd.Close()
}
