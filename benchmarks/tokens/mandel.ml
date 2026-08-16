let () =
  let w = 2000 and h = 2000 in
  let total = ref 0 in
  for y = 0 to h - 1 do
    let ci = (float_of_int y -. float_of_int h /. 2.0) *. 4.0 /. float_of_int h in
    for x = 0 to w - 1 do
      let cr = (float_of_int x -. float_of_int w /. 2.0) *. 4.0 /. float_of_int w in
      let zr = ref 0.0 and zi = ref 0.0 and i = ref 0 in
      while !i < 255 && (!zr *. !zr) +. (!zi *. !zi) <= 4.0 do
        let t = (!zr *. !zr) -. (!zi *. !zi) +. cr in
        zi := 2.0 *. !zr *. !zi +. ci;
        zr := t;
        incr i
      done;
      total := !total + !i
    done
  done;
  Printf.printf "%d\n" !total
