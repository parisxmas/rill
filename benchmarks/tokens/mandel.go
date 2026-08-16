package main

import "fmt"

func main() {
	w, h := 2000, 2000
	var total int64
	for y := 0; y < h; y++ {
		ci := (float64(y) - float64(h)/2.0) * 4.0 / float64(h)
		for x := 0; x < w; x++ {
			cr := (float64(x) - float64(w)/2.0) * 4.0 / float64(w)
			zr, zi := 0.0, 0.0
			var i int64
			for i < 255 && zr*zr+zi*zi <= 4.0 {
				t := zr*zr - zi*zi + cr
				zi = 2.0*zr*zi + ci
				zr = t
				i++
			}
			total += i
		}
	}
	fmt.Println(total)
}
