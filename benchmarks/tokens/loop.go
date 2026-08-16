package main

import "fmt"

func main() {
	v := make([]int64, 1024)
	for i := 0; i < 1024; i++ {
		v[i] = int64(i)
	}
	var acc int64
	for i := int64(0); i < 100000000; i++ {
		acc += v[i&1023]
	}
	fmt.Println(acc)
}
