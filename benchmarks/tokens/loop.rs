fn main() {
    let mut v = vec![0i64; 1024];
    for i in 0..1024 {
        v[i] = i as i64;
    }
    let mut acc = 0i64;
    for i in 0..100_000_000i64 {
        acc += v[(i & 1023) as usize];
    }
    println!("{acc}");
}
