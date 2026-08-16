let inf = 0x3FFFFFFFFFFFFFFF
let hsize = ref 0

let swap b i j =
  let x = b.(i) in
  b.(i) <- b.(j);
  b.(j) <- x

let hpush hd hn d v =
  hd.(!hsize) <- d;
  hn.(!hsize) <- v;
  let i = ref !hsize in
  incr hsize;
  while !i > 0 && hd.((!i - 1) / 2) > hd.(!i) do
    let p = (!i - 1) / 2 in
    swap hd !i p;
    swap hn !i p;
    i := p
  done

let hpop hd hn =
  decr hsize;
  hd.(0) <- hd.(!hsize);
  hn.(0) <- hn.(!hsize);
  let i = ref 0 and go = ref true in
  while !go do
    let l = (2 * !i) + 1 in
    if l >= !hsize then go := false
    else
      let c = if l + 1 < !hsize && hd.(l + 1) < hd.(l) then l + 1 else l in
      if hd.(!i) <= hd.(c) then go := false
      else begin
        swap hd !i c;
        swap hn !i c;
        i := c
      end
  done

let () =
  let n, m, s, t = Scanf.scanf " %d %d %d %d" (fun a b c d -> (a, b, c, d)) in
  let q = max m 1 in
  let eu = Array.make q 0 and ev = Array.make q 0 and ew = Array.make q 0 in
  for i = 0 to m - 1 do
    let a, b, c = Scanf.scanf " %d %d %d" (fun a b c -> (a, b, c)) in
    eu.(i) <- a;
    ev.(i) <- b;
    ew.(i) <- c
  done;
  let off = Array.make (n + 1) 0 in
  for i = 0 to m - 1 do
    off.(eu.(i) + 1) <- off.(eu.(i) + 1) + 1
  done;
  for i = 1 to n do
    off.(i) <- off.(i) + off.(i - 1)
  done;
  let cur = Array.copy off in
  let dest = Array.make q 0 and wt = Array.make q 0 in
  for i = 0 to m - 1 do
    let p = cur.(eu.(i)) in
    dest.(p) <- ev.(i);
    wt.(p) <- ew.(i);
    cur.(eu.(i)) <- p + 1
  done;
  let dist = Array.make (max n 1) inf in
  dist.(s) <- 0;
  let hd = Array.make (m + 1) 0 and hn = Array.make (m + 1) 0 in
  hpush hd hn 0 s;
  let ans = ref (-1) and go = ref true in
  while !go && !hsize > 0 do
    let d = hd.(0) and u = hn.(0) in
    hpop hd hn;
    if u = t then begin
      ans := d;
      go := false
    end
    else if d <= dist.(u) then
      for e = off.(u) to off.(u + 1) - 1 do
        let nd = d + wt.(e) in
        if nd < dist.(dest.(e)) then begin
          dist.(dest.(e)) <- nd;
          hpush hd hn nd dest.(e)
        end
      done
  done;
  print_int !ans;
  print_newline ()
