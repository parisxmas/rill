let () =
  let v = Array.make 1024 0 in
  for i = 0 to 1023 do v.(i) <- i done;
  let acc = ref 0 in
  for i = 0 to 99999999 do acc := !acc + v.(i land 1023) done;
  print_int !acc; print_newline ()
