type j =
  | Null
  | Bool of bool
  | Int of int
  | Str of string
  | Arr of j list
  | Obj of (string * j) list

let src = ref ""
let pos = ref 0
let len () = String.length !src
let cur () = (!src).[!pos]

let die msg i =
  Printf.printf "error at %d: %s\n" (i + 1) msg;
  exit 1

let rec ws () =
  if !pos < len () && (match cur () with ' ' | '\t' | '\n' | '\r' -> true | _ -> false) then begin
    incr pos;
    ws ()
  end

let plit lit v =
  let n = String.length lit in
  if !pos + n <= len () && String.sub !src !pos n = lit then begin
    pos := !pos + n;
    v
  end
  else die "unexpected character" !pos

let pint () =
  let i = !pos in
  let a = if cur () = '-' then i + 1 else i in
  let j = ref a in
  while !j < len () && (!src).[!j] >= '0' && (!src).[!j] <= '9' do incr j done;
  if !j = a then die "unexpected character" i;
  let v = int_of_string (String.sub !src i (!j - i)) in
  pos := !j;
  Int v

let pstr () =
  let b = Buffer.create 16 in
  let rec go () =
    if !pos >= len () then die "unterminated string" !pos
    else
      match cur () with
      | '"' -> incr pos; Buffer.contents b
      | '\\' ->
        incr pos;
        if !pos >= len () then die "unterminated string" !pos;
        (match cur () with
         | ('"' | '\\' | '/') as c -> Buffer.add_char b c
         | 'n' -> Buffer.add_char b '\n'
         | 't' -> Buffer.add_char b '\t'
         | 'r' -> Buffer.add_char b '\r'
         | 'b' -> Buffer.add_char b '\b'
         | 'f' -> Buffer.add_char b '\012'
         | _ -> die "bad escape" !pos);
        incr pos;
        go ()
      | c -> Buffer.add_char b c; incr pos; go ()
  in
  go ()

let rec parse () =
  ws ();
  if !pos >= len () then die "unexpected end of input" !pos
  else
    match cur () with
    | 'n' -> plit "null" Null
    | 't' -> plit "true" (Bool true)
    | 'f' -> plit "false" (Bool false)
    | '"' -> incr pos; Str (pstr ())
    | '[' -> incr pos; ws (); parr []
    | '{' -> incr pos; ws (); pobj []
    | c when c = '-' || (c >= '0' && c <= '9') -> pint ()
    | _ -> die "unexpected character" !pos

and parr acc =
  if !pos < len () && cur () = ']' then begin incr pos; Arr [] end
  else begin
    let v = parse () in
    ws ();
    asep (v :: acc)
  end

and asep acc =
  if !pos >= len () then die "expected ',' or ']'" !pos
  else
    match cur () with
    | ',' -> incr pos; ws (); let v = parse () in ws (); asep (v :: acc)
    | ']' -> incr pos; Arr (List.rev acc)
    | _ -> die "expected ',' or ']'" !pos

and pobj acc =
  if !pos < len () && cur () = '}' then begin incr pos; Obj [] end else pkey acc

and pkey acc =
  if !pos >= len () || cur () <> '"' then die "unexpected character" !pos
  else begin
    incr pos;
    let k = pstr () in
    ws ();
    if !pos >= len () || cur () <> ':' then die "expected ':'" !pos;
    incr pos;
    let v = parse () in
    ws ();
    osep ((k, v) :: acc)
  end

and osep acc =
  if !pos >= len () then die "expected ',' or '}'" !pos
  else
    match cur () with
    | ',' -> incr pos; ws (); pkey acc
    | '}' -> incr pos; Obj (List.rev acc)
    | _ -> die "expected ',' or '}'" !pos

let quote s =
  let b = Buffer.create (String.length s + 2) in
  Buffer.add_char b '"';
  String.iter
    (fun c ->
      match c with
      | '"' -> Buffer.add_string b "\\\""
      | '\\' -> Buffer.add_string b "\\\\"
      | '\n' -> Buffer.add_string b "\\n"
      | '\t' -> Buffer.add_string b "\\t"
      | '\r' -> Buffer.add_string b "\\r"
      | '\b' -> Buffer.add_string b "\\b"
      | '\012' -> Buffer.add_string b "\\f"
      | c -> Buffer.add_char b c)
    s;
  Buffer.add_char b '"';
  Buffer.contents b

let rec show = function
  | Null -> "null"
  | Bool b -> if b then "true" else "false"
  | Int n -> string_of_int n
  | Str s -> quote s
  | Arr xs -> "[" ^ String.concat "," (List.map show xs) ^ "]"
  | Obj fs -> "{" ^ String.concat "," (List.map (fun (k, v) -> quote k ^ ":" ^ show v) fs) ^ "}"

let () =
  src := (try input_line stdin with End_of_file -> "");
  let v = parse () in
  ws ();
  if !pos < len () then die "trailing content" !pos;
  print_endline (show v)
