package main

import (
	"fmt"
	"os"
	"strconv"
)

func main() {
	n := 5000
	dir := "/tmp/rill-io-small-" + os.Args[1] + "/f"
	for i := 0; i < n; i++ {
		s := "file " + strconv.Itoa(i) + " holds a little text and a number: " + strconv.Itoa(i*i)
		if err := os.WriteFile(dir+strconv.Itoa(i)+".txt", []byte(s), 0644); err != nil {
			panic(err)
		}
	}
	total := 0
	for i := 0; i < n; i++ {
		b, err := os.ReadFile(dir + strconv.Itoa(i) + ".txt")
		if err != nil {
			panic(err)
		}
		total += len(b)
	}
	fmt.Println(total)
}
