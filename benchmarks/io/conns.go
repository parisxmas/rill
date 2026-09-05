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
	msg := []byte(strings.Repeat("y", 64))
	done := make(chan int, 100)
	for i := 0; i < 500; i++ {
		go func() {
			fd, err := net.Dial("tcp", "127.0.0.1:"+port)
			if err != nil {
				panic(err)
			}
			reply := make([]byte, 64)
			got := 0
			for k := 0; k < 50; k++ {
				fd.Write(msg)
				n, _ := io.ReadFull(fd, reply)
				got += n
			}
			fd.Close()
			done <- got
		}()
	}
	acc := 0
	for i := 0; i < 500; i++ {
		acc += <-done
	}
	fmt.Println(acc)
}
