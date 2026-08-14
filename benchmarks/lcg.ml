let rec run seed k acc =
  if k = 0 then acc
  else run ((seed * 1103515245 + 12345) mod 2147483648) (k - 1) (acc + (seed mod 100))

let () = Printf.printf "%d\n" (run 42 100000000 0)
