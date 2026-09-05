package main

import (
	"bufio"
	"fmt"
	"os"
)

func main() {
	sc := bufio.NewScanner(os.Stdin)
	sc.Buffer(make([]byte, 1<<20), 1<<20)
	lines, bytes := 0, 0
	for sc.Scan() {
		lines++
		bytes += len(sc.Bytes()) + 1
	}
	fmt.Println(lines)
	fmt.Println(bytes)
}
