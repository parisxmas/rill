package main

import "fmt"

func produce(ch chan int, v, k int) {
	for i := 0; i < k; i++ {
		ch <- v
	}
}

func main() {
	k := 250000
	a, b, c, d := make(chan int), make(chan int), make(chan int), make(chan int)
	go produce(a, 1, k)
	go produce(b, 2, k)
	go produce(c, 3, k)
	go produce(d, 4, k)
	acc := 0
	for n := 0; n < 4*k; n++ {
		select {
		case v := <-a:
			acc += v
		case v := <-b:
			acc += v
		case v := <-c:
			acc += v
		case v := <-d:
			acc += v
		}
	}
	fmt.Println(acc)
}
