package main

import (
	"bytes"
	"fmt"
	"os"
	"strconv"
	"strings"
)

func main() {
	n := 2000000
	var b strings.Builder
	for i := 0; i < n; i++ {
		b.WriteString("line number ")
		b.WriteString(strconv.Itoa(i))
		b.WriteByte('\n')
	}
	path := "/tmp/rill-io-big-" + os.Args[1] + ".txt"
	if err := os.WriteFile(path, []byte(b.String()), 0644); err != nil {
		panic(err)
	}
	back, err := os.ReadFile(path)
	if err != nil {
		panic(err)
	}
	fmt.Println(len(back))
	fmt.Println(bytes.Count(back, []byte{'\n'}))
}
