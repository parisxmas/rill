fn main() {
    let (w, h) = (2000, 2000);
    let mut total: i64 = 0;
    for y in 0..h {
        let ci = (y as f64 - h as f64 / 2.0) * 4.0 / h as f64;
        for x in 0..w {
            let cr = (x as f64 - w as f64 / 2.0) * 4.0 / w as f64;
            let (mut zr, mut zi) = (0.0f64, 0.0f64);
            let mut i: i64 = 0;
            while i < 255 && zr * zr + zi * zi <= 4.0 {
                let t = zr * zr - zi * zi + cr;
                zi = 2.0 * zr * zi + ci;
                zr = t;
                i += 1;
            }
            total += i;
        }
    }
    println!("{total}");
}
