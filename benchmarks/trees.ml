(* Idiomatic OCaml: Leaf is an immediate (unboxed) constructor, Node is a
   minor-heap block — OCaml's generational GC is excellent at this workload. *)
type tree = Leaf | Node of tree * int * tree

let rec build d = if d = 0 then Leaf else Node (build (d - 1), d, build (d - 1))

let rec check = function
  | Leaf -> 1
  | Node (l, _, r) -> 1 + check l + check r

let () =
  let acc = ref 0 in
  for _ = 1 to 10 do
    acc := !acc + check (build 18)
  done;
  Printf.printf "%d\n" !acc
