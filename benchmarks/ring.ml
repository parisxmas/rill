(* 100k lightweight fibers chained by rendezvous channels, using OCaml 5
   effect handlers — the canonical way to hand-roll what Rill and Go ship as
   built-in runtimes (~50 lines of scheduler). *)
open Effect
open Effect.Deep

type chan = {
  sendq : (int * (unit, unit) continuation) Queue.t;
  recvq : (int, unit) continuation Queue.t;
}

type _ Effect.t +=
  | Spawn : (unit -> unit) -> unit Effect.t
  | Send : chan * int -> unit Effect.t
  | Recv : chan -> int Effect.t

let runq : (unit -> unit) Queue.t = Queue.create ()
let new_chan () = { sendq = Queue.create (); recvq = Queue.create () }

let rec run_fiber (f : unit -> unit) =
  match_with f ()
    {
      retc = (fun () -> ());
      exnc = raise;
      effc =
        (fun (type a) (eff : a Effect.t) ->
          match eff with
          | Spawn g ->
              Some
                (fun (k : (a, unit) continuation) ->
                  Queue.push (fun () -> run_fiber g) runq;
                  continue k ())
          | Send (c, v) ->
              Some
                (fun (k : (a, unit) continuation) ->
                  if Queue.is_empty c.recvq then Queue.push (v, k) c.sendq
                  else begin
                    let rk = Queue.pop c.recvq in
                    Queue.push (fun () -> continue rk v) runq;
                    continue k ()
                  end)
          | Recv c ->
              Some
                (fun (k : (a, unit) continuation) ->
                  if Queue.is_empty c.sendq then Queue.push k c.recvq
                  else begin
                    let v, sk = Queue.pop c.sendq in
                    Queue.push (fun () -> continue sk ()) runq;
                    continue k v
                  end)
          | _ -> None);
    }

let rec schedule () =
  match Queue.take_opt runq with
  | None -> ()
  | Some t ->
      t ();
      schedule ()

let () =
  let first = new_chan () in
  run_fiber (fun () ->
      let rec chain n inp =
        let next_c = new_chan () in
        perform (Spawn (fun () -> perform (Send (next_c, perform (Recv inp) + 1))));
        if n = 1 then next_c else chain (n - 1) next_c
      in
      let last = chain 100000 first in
      perform (Spawn (fun () -> perform (Send (first, 0))));
      Printf.printf "%d\n" (perform (Recv last)));
  schedule ()
