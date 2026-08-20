# Rill Valley

A walkable low-poly valley at dusk, computed by Rill and rendered through
WebGPU. The terrain, its mesh, where every pine, crystal and grass tuft
stands, the walking camera with gravity and jumping, and the view and shadow
matrices are all computed in `world.rill`, compiled to WebAssembly. `host.js`
owns what a wasm module cannot: the promise-based device setup, the WGSL
shaders, and the render passes — a shadow pass under a fixed golden-hour sun,
a 4x MSAA forward pass with procedural sky and fog, instanced pines, crystals
and wind-swayed grass, Gerstner-wave water with shoreline and crest foam, and
a compute-shader swarm of fireflies.

The demo sizes itself to the machine. The adapter's description picks a
starting quality tier; then measured frame times take over, and the world is
regenerated denser or sparser — grid, forests, grass, fireflies, shadow map,
wave count, pixel ratio — until the display's own frame budget is spent.
Rill rebuilds the whole valley in about four milliseconds, which is what makes
re-asking cheap: the terrain is sampled once per lattice corner rather than
once per cell corner, and a candidate tree or crystal is rejected on its seed
before the ground under it is ever worked out — the ground being twelve sines
deep. When the adapter offers `timestamp-query`, the HUD shows
what a frame actually costs the GPU.

```sh
rill build world.rill -o world --target wasm32-wasip1   # writes world.wasm + world.js
python3 -m http.server 8000        # from this directory
# open http://localhost:8000 in a browser with WebGPU
```

The valley itself is `valley.rill`, and it is compiled twice. `world.rill` is
fifty-six exports over it and becomes `world.wasm` for the browser;
`server.rill` is a WebSocket server around it and becomes a native binary that
several browsers can walk about in at once. Neither of them decides anything
about what the valley *is* — that is all in the one file, which is the only
reason a server and a client can agree about where a round went.

`world.js` is the generated loader — the WASI shim, instantiation and init —
so `host.js` here is nothing but the scene: passes, pipelines and tiers. The
shaders themselves are in `shaders/`, one WGSL file per pipeline, fetched in
parallel at startup. They are not standalone: every one is written against
`uniforms.wgsl`, and all but the shadow pair against `common.wgsl` too — the
shadow pass writes the shadow map and so must not be able to see it. What each
may reach is stated in one place, the `SHADERS` map at the top of `host.js`.

Click to capture the mouse; WASD to walk, space to jump, shift to sprint.
Trees and crystals are solid — you slide around them, not through them — and
the wind that moves the water moves the pines and the grass, because all
three read the same two numbers Rill computes.

Six textures ride along. Two are alpha atlases: `pine-twig.png`, the needle
cutout the branch cards carry, and `plants.png`, a 2×2 grid of undergrowth —
two tufts of grass, a fern frond, a nettle leaf. Four are tiled: `pine-bark.jpg`
up and around the trunks, and `grass-ground.jpg`, `rock-face.jpg` and
`shore-sand.jpg` for the ground. The ground's three are tiled straight from
world x/z, which is why the terrain still needs no texture coordinate of its
own; rock is also projected sideways and crossfaded in by steepness, because
flat x/z alone smears stone into vertical streaks down exactly the cliffs it
was meant to describe. All six come from Poly Haven's CC0 library —
`pine_tree_01`, `pine_bark`, `leafy_grass`, `rock_face_03`, `coast_sand_03`,
and `grass_medium_01` / `fern_02` / `nettle_plant` for the plant atlas — 197 KB
between them against 30 KB of wasm. Mip levels are made at load by decoding
each file again at each size, because alpha tested foliage without them
shimmers from a few trees away.

One square card serves all four plants. Each was pasted into its atlas cell at
its own aspect and stood on the cell's floor, so a tuft comes out low and wide
and a fern comes out tall and narrow from the same four corners — the rest of
the card is transparent and gets discarded. Which cell an instance draws is
rolled against the size Rill gave it: the taller half of the meadow is more
likely to be a fern or a nettle than a tuft, but only likely, because a meadow
sorted strictly by size reads as sorted.

None of the photographs is used as it was shot: they are daylight and this
valley is dusk. Each is split — brightness drives the variation, hue is folded
in a third of the way, and the face's own colour still decides what the valley
looks like. The brightness is then read a second time as a height, and the
normal bent along its gradient: rock strongly, sand less, grass barely. That
needs no normal map and no tangent frame, only the screen-space derivatives of
the height and of the surface position, which is what a tangent frame would
have been for.

Colour is per corner, and so is the normal. The bands — sand, meadow, rock,
snow — are what makes the valley read as low-poly, and they stay; but they are
weighed rather than chosen. A threshold puts the whole of a snowline at one
triangle's edge, and that edge is a metre long. The shader reads the weights
back out of the colour so the textures crossfade on the same gradient the
colours do, and the three of them sum to one — halfway between meadow and sand
is fully both, not half of neither, which is where the texture used to vanish.

The normal is a lighting question rather than a style one, so Rill takes each
corner's from the slope of its own height lattice — two subtractions, not a
cross product. Four cells meeting at a corner agree about it, and the light
crosses the seam.

The lake is a sum of plane waves, and it used to look it: five or six of them
inside two thirds of a radian of each other, on wavelengths in near-halves,
all starting at phase zero. Waves that parallel and that harmonic add up to one
shape repeated, which is the corduroy the water wore. The spectrum is broad
now — direction fans wider for the shorter waves, because a swell holds its
line and chop does not, and the offsets are sines of the golden angle, which
never come back round on themselves for any count of waves. Wavelengths fall by
a jittered 0.72 rather than a half, and every wave starts somewhere else.

What actually makes it read as water, though, is that the sea comes in sets: a
slow noise field drifting across the lake scales every wave at once, so there
are patches of chop and patches nearly glassy. Without it the same amplitude
covers the whole surface, and no amount of spectrum saves that.

## Other people

```sh
rill build server.rill -o valleyd && ./valleyd          # holds one valley
python3 -m http.server 8000                             # and the page beside it
# open http://localhost:8000/?join   — or ?join=ws://a.box:8091/ for someone else's
```

Without `?join` nothing below happens and the valley is this tab's alone, which
is what a static page gets. With it, the world belongs to the server: the
crystals, the rounds in the air, the craters and the shark are that process's,
and a browser is told about them.

The server is one strand per connection and one hub that owns the world, which
is what makes it safe without a lock anywhere — the world block is reached from
exactly one strand and everything else is a channel. A tick is a message on the
same channel as the key presses, so an input either lands before a tick or
after it and never inside one. Twenty ticks a second, with `sleep_ms` parking
the ticker rather than holding a worker; the deadline is carried rather than
recomputed, so a tick that woke up late is followed by one that sleeps less.

What goes over the wire is text, and deliberately: the whole exchange is
legible in a terminal with `websocat`, which was worth more while it was being
got right than the bytes were. Twenty times a second every client is told the
world clock, where the shark is, and where everybody stands; and as they
happen, every round fired, every fireball thrown, every crater and every
crystal that came apart. Those events are not recorded twice — each of those
things already lives in a ring with a cursor that counts up, so what a client
has not been told is exactly the difference between the cursor now and the
cursor at the start of the tick.

A connection that ends says so — the read comes back empty, the player is
taken out of the world, their block is freed and their id is retired, so a
round still in the air that carries it cannot be credited to somebody who
arrives later. A connection whose far end fell asleep, lost its network or was
unplugged says nothing at all, and TCP will not mention it for hours: the read
simply never returns. The only thing that can tell those apart is the silence,
so the tick drops anybody who has not been heard from for ten seconds — a
client sends its keys every frame it draws, so that is two or three hundred
messages that did not arrive. Ten seconds of the *world's* clock, which is a
fraction slower than the wall's, since the world advances a fiftieth of a
second per tick and a tick takes however long the operating system's timers
take.

The socket of somebody dropped that way is deliberately left alone. A strand is
parked on that descriptor inside the poller; closing it from another strand
does not wake it, and the number could be handed to the next connection while
the old registration is still armed, which is a great deal worse than a
descriptor held open until TCP gives up on it. What matters is that they are
out of the valley, and that happens at once.

A client does not simply draw what it is sent. It walks itself as well, on the
same `walk` the server runs, so that turning and moving answer the key rather
than the round trip; what comes back is eased in over a few frames unless it is
metres out, which is what being thrown by a blast looks like from here. Aim is
never corrected — a server that moved your crosshair while you were using it is
worse than one that lets you look wherever you like. Between snapshots the
world keeps moving under `step_visuals`, which is everything that moves and
nothing that decides: rounds fly and hit nothing, fireballs arc and leave no
crater. Whether they hit anything is the server's answer, and it arrives.

The valley everyone stands in is generated, not shipped: the server says how
many pines, crystals and tufts of grass, and each client builds the same one
from the same three numbers, because the placement is a deterministic walk over
a hash. The terrain *mesh* resolution is still the client's own — a slow
machine draws a coarser valley and stands on exactly the same ground, since
every physical question is asked of the height function rather than of the
mesh.

Nothing a client sends is believed. Every number is checked finite, because
`str_to_float` will hand back a NaN for `"nan"` and NaN compares false against
everything — a player at a NaN position is hit by nothing, seen by nothing,
and always far enough away to be worth throwing at. Pitch is held to the same
limits `walk` holds it to. A shot's direction is made a unit vector on
arrival: it is multiplied by the muzzle velocity, so a direction a million
long is otherwise a round at a hundred and fifty billion metres a second,
which crosses the valley inside one step and takes out every crystal along a
line through the world.

Each connection is allowed a hundred and fifty messages a second — a browser
sends its keys sixty times a second and a ping once, so that is well above
anything honest — and over that the reader
sleeps until the second is up rather than reading on. The cap on the page
matters as much as the number here: it used to send one input per *frame*, and
on a 144 Hz screen that is 144 a second — over the allowance, so a fifty-fourth
of every second went unread, waited in the kernel, and everything behind it
waited too. The player's own ping came back reading half a minute, because a
ping is a message like any other and it was queued behind the backlog. That is not tidiness: the
hub takes its tick off the same channel as the key presses, and against an
unbounded sender the tick never wins at all. It has to be a sleep and not a
drop, because reading is most of what a message costs: the frame walk and the
unmask happen before anything can decide a line is unwanted, so a client
sending a million lines a second was still being read a million times a second
and an honest player got no snapshots for as long as it went on. Asleep, the
frames stay in the kernel buffer, the window fills, and the sender's own writes
block. Sixty-four sockets may be open at once, whichever of them ever say
anything, and no more than sixteen from one address.

Behind a proxy the address comes from the `X-Real-IP` header rather than from
the socket, and only when the socket is the loopback — which on a machine where
nothing else can reach the port means nginx. Without that, `wss://` would make
every player look like 127.0.0.1 and sixteen of them would fill the valley for
everybody. With it, a header from anywhere else is ignored, which is the whole
of the security in it: a client that could name its own address would invent a
new one per connection.

A name may be changed once every three seconds, and only to a different one.
Every accepted name goes out to everybody, so that is the one thing a single
client can make the server say to all the others, and at ninety a second times
sixty-four people it is the cheapest amplifier in the protocol.

Writing to a socket whose far end has gone is an error here and not a signal —
`SO_NOSIGPIPE` on BSD, `MSG_NOSIGNAL` per send on Linux. It has to be said out
loud because Rust's runtime turns SIGPIPE off before `main` and a Rill binary's
main is generated code that never goes through it, so until this was set any
client that walked away at the wrong moment killed the server outright.

A connection has five seconds to get its handshake in, measured against the
clock rather than per read, so a client dribbling a byte at a time cannot
extend it. That needs `sock_wait`, which the language grew for this: closing
the socket from another strand would not have worked, because the reader is
parked on that descriptor inside the poller and closing it from elsewhere does
not wake it.

Sixty-four connections at once, and sixteen from any one address — the whole
cap alone is no defence against one machine opening all sixty-four of them.
Sixteen is generous for a household or an office behind one address and still
leaves most of the valley to everybody else.

Other people are drawn as people. The model is Quaternius' mannequin from the
Universal Animation Library (CC0) — fifty-three joints, 13,744 triangles — and
seven of that library's clips: standing, walking, running, crouching, firing,
taking a round and going down. `tools/bake_figure.py` takes the glTF and
writes `figure.mesh`, 383 KB, and three facts about the source are what make it that small rather
than a megabyte and a half. Nothing in the rig is ever scaled. Exactly one
joint translates — the hips — so every other joint keeps its bind translation
and a frame is a root position and a rotation each. And the clips are authored
on a thirty-a-second grid already, so resampling onto one loses nothing.
Rotations go down as snorm16, which is a hundredth of a degree.

The skinning is split the way the rest of this is: the page works out one
matrix per joint per person — fifty-three small matrix products each, which is
nothing for eight people — and the shader is four matrices and their weights.
The joint matrices arrive already in world space, so there is no model
transform, no bind pose and no per-instance rotation in the vertex stage.

Which clip somebody is playing is the *server's* answer, not a guess made at
this end from how fast a figure seems to be sliding: it has the keys and the
health, and two people watching the same person should not disagree about
whether they are running. Firing is the exception, because it is an event
rather than a state — a round leaving somebody's rifle plays the shot over
whatever their legs are doing. Being hit is the other, and it is not in the
protocol at all: a health that has gone down since the last snapshot without
reaching zero is somebody taking a round, which is the same fact the server
would have had to send, arriving for nothing.

Holding the right button aims: the world narrows to five times, the rifle comes
to the eye and straightens, the sway drops to a quarter, and past nine
tenths of the way the rifle goes and the scope becomes the screen: a clear
circle two thirds of the screen high, black around it, and a duplex reticle with a
mil scale in the middle of it — thick posts to carry the eye in, a thin cross
to aim with, and ticks two mils apart to hold over with. The page's crosshair
fades out on the way up; two opinions about where the middle is, is one too
many. Looking down the tube of the rifle's own scope was
honest and useless — the far lens is a centimetre across and the eye is twenty
behind it, so the sight picture was a keyhole. The zoom is one number written into the player's block and read by
`camera`, so the projection is still Rill's; the mouse is slowed by the same
factor on the page, or the same movement of the hand covers five times the
angle and nothing can be held on target.

Being killed leaves you on the ground for ten seconds. The camera goes down
with the body — the eye falls to a third of a metre, the horizon rolls a
quarter turn, and it goes over on the side the last round threw the head, so
the fall finishes the motion being shot started — and the screen says YOU ARE
DEAD with the seconds counting down under it. The rifle goes with them and onto the
ground — no viewmodel while you are down, because a gun held out in front of a
camera lying in the grass is the one thing on that screen still behaving as
though nothing had happened, and in its place one lying a pace ahead of where
you fell, on its side and turned a little off your facing. Everybody else's
does the same rather than staying gripped in a hand the die clip has just put
on the ground. Somebody on the ground does not walk, does not look about and
cannot fire, all three in `walk`, which both ends run. Getting up is a decision and so the server's; the clock is kept by both,
because a client needs it for the camera and for the number. Alone in your own
valley none of this happens: there, running out of health is the end of the
thing and there is a screen that says so.

Being shot throws the camera about — a roll mostly, settling over four hundred
milliseconds — and that is noticed rather than announced: health only ever goes
down, so the frame it goes down on is the frame this player was hit, whether
the damage came from a round, a blast or a server saying so. It goes on the
camera and not on the yaw and pitch, because what you were aiming at should
still be what you are aiming at when it settles.

What everybody holds is the same `rifle.mesh` the viewmodel draws — one rifle
in this valley rather than two — instanced once per person off the hand
joint's world matrix times a grip. The grip is worked out at bake time rather
than by eye, and against the armed idle rather than the bind pose: a bind pose
has the palms turned down, every armed pose turns the wrist ninety degrees to
grip, and a rifle squared up against the bind pose lies on its side for the
whole game. The barrel goes where the figure faces, up is world up, and the
frame is written into the header with the index of the joint it hangs off. So
it is level when it is being aimed, follows the wrist everywhere else, and the
page knows nothing about the rig beyond one number.

Walking, running and crouching take their legs from those clips and everything
from the spine up from the armed idle, because the library has no armed walk:
the arms in those three were animated empty and swung like it. It is a mask on
the skeleton rather than two clips faded together, and it carries the pelvis —
whose rotation is what pitches a running body forward, taking the whole torso
and the rifle with it. Its children stay behind, so the legs still run on an
upright pelvis, and the stride's rise and fall is a translation and is not
masked, so a crouching player is still down and a running one still bobs. The
mask is in `figure.mesh` a joint at a time, because the rule has an exception
in it and the place to know about that is the tool reading the rig.

## What a client is told

The snapshot used to carry everybody's position to everybody. Nothing a browser
can do fixes that — a client that has been told where somebody is can draw them
through a hill — so the answer is in the server: it works out, per reader, who
has a line of sight to whom, and a player behind the terrain arrives with their
position blanked and a zero on the end of their block.

They stay in the roster. Names, health and the two counts are who is in the
valley, which is not a secret from anyone standing in it; what is withheld is
*where*. It is the only one of the three defences here that a modified client
cannot work around, because it is not a rule it is asked to follow.

The line is eye to eye, so it is the same question both ways round, and the
ground may come a third of a metre above it before it counts as blocking —
somebody on a crest reads as visible rather than flickering. Trees and crystals
are not asked: they are thin, and hiding somebody behind a sapling costs more in
shots that should have landed than it takes from anybody cheating.

The cost is the one-line broadcast, which was the cheapest thing in the loop:
it is now one string per reader. Measured with everybody running about, it does
not show — 20 snapshots a second either way, and the server at one per cent of
a core with twenty-four players in the valley. What it buys depends entirely on
the terrain, and this terrain is a bowl: about one player in twenty is blanked
at any moment. In a valley with more cover it would be most of them.

## Whose name is whose

Everybody gets a token the first time they arrive. The browser keeps it, it
comes back in the query on the next visit, and the name it took is refused to
everybody else — including while its owner is away. Two files next to the
binary hold all of it: `who.txt`, one `<token> <name>` per line, and
`banned.txt`, one token per line, read on every knock so a ban takes hold
without a restart.

It is not an account. It is never typed, it cannot be recovered, and anybody
who copies the string out of a browser is that player. What it buys is a name
that stays yours and a way to say that one particular person is not welcome —
neither of which is possible when the only handle is an address.

A round has to go roughly where its owner says they are looking — twenty
degrees of slack, which is a frame's lag on a fast turn and nowhere near enough
to shoot behind yourself. Nothing a server can do stops somebody aiming
perfectly; this makes them turn to do it, and turning is the part everybody
else can see.

What a round costs depends on where it lands: the head ends it, the body takes
a third, the legs a tenth — three spheres tested from the top down, since the
smallest is worth the most. Whoever is hit is told who hit them and for how
much, low and in the middle of their screen, and whoever hit them is told what
they did — in green rather than red, with a skull on it if that was the end of
them. Whether it was is the server's to say and rides with the hit, because the
snapshot that says somebody is down says nothing about who put them there. It
goes out as a ring and a cursor like everything else that happens between two
ticks, and to everybody rather than to the one it happened to, because four
numbers are cheap and a valley
where you can see what is happening to the person beside you is a different
place from one where you can only see what is happening to you.

A round stops at the person it hit. On the server that is `try_hit_player`; on
a client there are no player blocks but its own, so the page writes down where
it is drawing everybody and the rounds stop there. No health lives in that
table and nothing else reads it: what a hit costs is still the server's word.
The positions are the interpolated ones the figures are drawn at, so a round
stops at the body you can see rather than at one a tick behind it.

Placing a figure is π − yaw and not yaw + π. The second mirrors rather than
turns, so everybody walked at a right angle to where they were aiming, and on
a mannequin that is symmetric until it moves that is invisible until you watch
somebody walk.

Everybody is asked who they are before they connect, and their name is drawn
over their head: sent once, kept by the server, handed to whoever walks in
next. It is cut down to letters and digits, eight at most — a space would split the
line it travels on — and anybody who says nothing is `player7`. It also has to
be free: a taken name gets a number, which eats into the eight rather than
pushing past it, and the client is told what it ended up with rather than
being refused after its world is already built. The tags are elements over the canvas rather than geometry in it,
placed with the same view-projection matrix the valley is drawn with; they are
not occluded by the terrain, which is a choice.

Down the left is everybody in the valley: their colour, their name, and a bar
for how much of them is left. The tag over a head answers "who is that"; the
roster answers "who am I in here with", which is a different question and not
one that can be answered by turning round. Top right is the round trip to the
server, once a second — answered from the hub rather than the reader strand,
so what it measures is how long this server takes to get round to you rather
than how fast the socket is.

The crystals do not throw fire in the shared valley for the moment. A meteor
is aimed at where somebody is standing, and standing about is most of what
people do while there are others to look at; `meteors()` in `server.rill` is
the switch, and a browser alone in its own valley still gets them.

It is still a demo server, and nothing here has been through anything harsher
than an afternoon of being shot at from the same machine.
The crash this used to die of is fixed — it was a `select` in a loop leaking
its stack, which `CHANGELOG.md` tells properly — but a demo server that has
been shot at for an afternoon is not a server that has been shot at, so do not
put it on the open internet.

`check.rill` is the same shared world with neither a browser nor a socket:
three players in one valley, walking, shooting each other and leaving. It is
run by the test suite, which is the only way any of this is checked
automatically — a server that holds a port and a renderer that wants a GPU are
both things a test suite cannot have.

## The rifle

Poly Haven's `bolt_action_rifle_7_62`, CC0, baked down from a glTF into one
file: welded, indexed, and quantized — position as three unorm16 across the
model's own bounding box, normal as three snorm8, the atlas coordinate as two
unorm16. Sixteen bytes a vertex against thirty-two, which with the indexing
takes it from 1.9 MB to 423 KB for 19,913 triangles. Its two materials sit
side by side in one 1024×512 atlas, which is why it is one draw call.

It is placed by aiming rather than by carrying: the scope's own optical axis on
the view axis, the barrel pointing where the eye points, and the butt behind
the eye where a shouldered stock is. That needs a frustum of its own — a near
plane at a centimetre, and a tighter field of view than the valley's, because a
wide angle applied to something at arm's length stretches it into a caricature.
The world's matrix cannot do either: a near plane that close would throw away
the depth precision the valley needs across four hundred metres. So the gun is
projected by hand and its depth squeezed into the front hundredth of the buffer,
which is nearer than anything the camera can walk up to.

The shot is Rill's. `fire` puts a round in a ring of thirty-two at 420 m/s, and
`frame` flies it: gravity at 9.81 and drag taking a fraction of the speed away
every second, which is why a bullet's drop grows faster than the schoolbook
parabola says — it is slowing down the whole way, so it spends longer falling
over the second half of its flight than over the first.

The streak is built in screen space rather than as a billboard stretched along
the velocity. The obvious way is wrong here: this is a first-person view, the
round flies almost straight away from the camera, and a quad pointing away from
the camera projects to nothing at all. In screen space the same round comes out
as a bright point going away from you and as a streak when it crosses your
view, which is what it looks like.

A round that reaches a crystal takes it out of the instance buffer — its scale
goes to zero, which is how the host stops drawing it without the array changing
length — and leaves eight shards in its place, thrown outward and tumbling on
all three axes. They keep the colour of what they came from, fall, land, and
stay: a shard that reaches the ground has its velocity and its spin set to zero
and its height snapped to the terrain, which is the same thing the walking
camera does when it lands. The hit test is against the segment the round
covered this frame, not against where it happens to be at the end of one —
seven metres pass between frames, and a point test would go through anything
narrower than that.

Since the crystals are not all one thing, what they leave is not all worth the
same. There are three in the valley and the colour is the whole of the tell:
amber is three in five of them and mends three per cent of a player, cyan is
one in five and mends six, and violet — one in six — mends twelve. Rarity and
value in the same order, which is what makes a colour worth reading at two
hundred metres: the rare one is a reason to cross the valley rather than the
same crystal in another paint. It averages 5.19 per cent against the flat five
that every crystal used to be worth, so the valley hands out what it always
did; only where it comes from has moved. The colour is read once, at the break,
and a chip carries a number from then on — a palette that changes should change
what the valley looks like and not what it heals.

## The ammunition

Ten rounds go in it, and there are sixteen boxes lying about the valley to fill
it again from. The box is Poly Haven's `ammo_box`, CC0, by DanKit — the same
library the rifle came out of, and photoscanned the same way: olive drab,
yellow stencils, worn paint and rust. `tools/bake_prop.py` welds its five nodes
— a body, a handle, a latch, two rings — into one buffer in the rifle's own
format, 3,978 vertices and 4,382 triangles in 88 KB. The published diffuse is
1k and is halved here: nothing gets nearer to one of these than the reach that
picks it up, and 512 is already more texels than the box has pixels.

It is drawn out of the shard buffer. A box on the ground *is* a shard as far as
Rill is concerned — the same twenty floats, a position, an orientation, a scale
and a time to come back at — so the front sixteen slots are boxes and every
chip of broken crystal goes in what is left. Two draws over one buffer, split
by `firstInstance`, and nothing is copied to make the second one possible. The
scale is the world's opinion of how big to draw the thing rather than a number
in the shader: sixteen boxes over a hundred and fifty metres, and at the twenty
six centimetres a real one measures, a box in that grass is something you walk
past. It is drawn at one and six tenths of life.

Only the yaw is used. A shard tumbles on all three axes because it was thrown;
a box was put down, and a box lying on its corner reads as a bug.

The bang is a Mosin Nagant, which is the rifle in the hands, recorded outdoors
with its own echo off the hills already in the tail — from Vincent Sevedge's
firearm recordings, CC BY 3.0. Two and a half seconds, 25 KB, decoded once and
played from the same buffer with a hair of detune and level each time, because
the same file twice in a row is the one thing that gives a sample away.

Being shot has a voice: four of qubodup's CC0 recordings of himself straining,
from OpenGameArt, a second each end to end in one file and played by offset the
way the glass is. Four is enough that a burst does not sound like one man on a
loop, and it is louder for a bigger hit — a leg and a head are different things
to have happen to you and should not sound the same.

A crystal going is three glass breaks from OpenGameArt's CC0 breaking pack,
laid end to end in one file a second apart and played by offset: one fetch and
one decode for three, and three is enough that a row of crystals does not sound
like a loop. It is quieter with distance and panned to where it happened, which
matters when there are three of them in front of you and only one of them
broke. Rill counts the breaks and writes down where the last one stood; the
host has no other way of knowing, because the hit happens inside a frame it
only calls.

The bolt is worked between shots, and until it is closed the trigger does
nothing — which is the whole of what a bolt action is. It is four movements
that do not overlap: lift the handle, draw it back, run it forward, turn it
down. Each gets its own slice of the cycle, eased, because a bolt is worked by
a hand rather than by a servo: it starts and it stops. Drawing back lifts the
round out of the magazine, throws it up and to the right and lets it fall
tumbling — the extractor's own arc, which is why brass leaves a bolt gun over
your shoulder rather than in front of you.

Which vertices move is a flag baked into the spare fourth component of the
position: one for the bolt's two thousand-odd vertices, a half for the round's
four hundred, zero for the rifle proper. The position was already being
unpacked from unorm16, so the flag costs the file nothing at all.

The people are Quaternius' mannequin and seven clips from his Universal
Animation Library, both CC0, by way of OpenGameArt. Nothing of it is shipped
as it came: `tools/bake_figure.py` keeps the mesh, the fifty-three joint
skeleton and the seven clips, and leaves the other forty behind.

The theme is pauliuw's *The Field of Dreams* from OpenGameArt, CC0 — eighty
seconds that begin and end in silence, which is what makes it loop without a
seam: no crossfade, no loop points, just the tail running into the head the way
the piece was already written. It starts on the click that captures the mouse,
because a browser will not start audio before a gesture and that is the first
one there is, and it fades up over four seconds rather than cutting in. That
matters more than it sounds like it should: the piece opens on a held chord,
and a held chord at full volume the instant you click reads as a jingle rather
than as somewhere you have arrived. **M** turns it off and on.
