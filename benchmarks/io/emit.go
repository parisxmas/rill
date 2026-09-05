package main

import (
	"bufio"
	"os"
	"strconv"
)

func main() {
	w := bufio.NewWriterSize(os.Stdout, 64*1024)
	defer w.Flush()
	for i := 0; i < 2000000; i++ {
		w.WriteString(strconv.Itoa(i))
		w.WriteByte('\n')
	}
}
