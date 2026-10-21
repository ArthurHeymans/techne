# The node protocol

`techne node` (also `techne-node`) serves MessagePack frames on standard
input and output. Each frame starts with its byte length as a big-endian
`u32`. A session relays the same stream to its daemon. This is separate
from nREPL, which uses bencode and has not changed.

Before any requests, each end sends a framed map `{"version": 1}` and
checks the other's version. A mismatch closes the connection with a
version error; there is no negotiation or fallback to unversioned nodes.
The exchange times out after a minute, as it includes the transport
starting (an ssh connecting and authenticating). `node-connect` starts the
transport without waiting; a failed exchange is reported by the first
operation and every later one. The node also reports it on standard
error. Client and node must be upgraded together, including session
daemons already running.

Requests carry an id. Replies carry that id and may arrive out of order;
a request with id zero is a notification and receives no reply.
`crates/techne-node/src/protocol.rs` defines the messages.

## Data and remote values

Data still crosses as `write`'s text, read back at the other end. Both
client arguments and node results use `Vm::to_written_data`: booleans,
characters, all numbers, strings, bytevectors, symbols, keywords, the
empty list, pairs (including improper lists) and vectors are data.
Dicts, records, procedures and other objects remain remote values.

This conversion lives beside `techne_vm::data::Data`, sharing its byte
accounting and the reader's nesting limit, currently 1,000. It does not
convert through `Data`: that owned model admits dicts but excludes
improper lists, integers outside 64 bits, ratios and complex numbers.
Narrowing the node to that model would stop existing arguments working.

One data value has a 1 MiB budget, counting 64 bytes per part plus its
text or byte contents; the written form must also fit 1 MiB. Shared
parts count each time they would be written. List tails do not add
nesting. An iterative walk detects cycles and checks sizes before listing
container elements, so neither cycles nor a large or deeply nested
structure can make classification run without a bound.

A result outside these limits is still a remote value. Its preview is
`#<value past data limits>`; other remote values have a written preview
cut short after about 4 KiB. A preview need not be a readable datum.
The node holds the value until the client's handle is collected or the
connection ends. It can still be passed back to a procedure on that node.
A local argument outside the data set or its limits raises an error:
there is no local object for a remote handle to name.

## Frame limits

Frames may be at most 16 MiB in either direction. Serialization stops at
that bound before writing any bytes to the stream. An oversized reply
is replaced by a small error reply with the same id; an oversized client
request fails locally. Neither refusal makes the connection unusable.
Incoming frames with a larger length are refused before allocating their
body, and the connection ends.

These are data-conversion and framing limits, not limits on evaluation:
a program can still allocate large values or print large output before
its reply is classified and framed. Pending requests and held remote
values are not given an aggregate memory limit by this protocol.
