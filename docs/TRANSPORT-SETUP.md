# Cancellable relay setup

The outbound WSS transport has a crate-internal `SetupControl` for a continuing
pilot's bounded connection attempts. It supplies an absolute monotonic deadline,
a shared cancellation flag and an explicit completion flag. This is a transport
primitive; it does not itself reconnect, authorize a new peer or renew text TTL.

The normal public `connect` and `relay-check` paths retain their existing setup
behavior. Controlled setup caps socket attempts and TLS/rendezvous I/O to the
supplied budget. TCP address attempts are bounded to 200 ms each, at most eight
resolved addresses. Cancellation is checked before connecting and during I/O.
OS DNS resolution itself cannot be cancelled or bounded by these socket settings;
if it eventually returns after cancellation, no new connection is initiated.

The setup budget remains in force through the caller's application peer hello.
Only after that validation does its owner mark setup complete. Established links
then use their normal I/O behavior; dropping `RelayLifetime` shuts down the socket
and cancels blocked stream workers. Already executing OS calls cannot be recalled.

Errors distinguish transport loss/timeouts/capacity from TLS/access/peer or
protocol rejection. Wrong certificates and access credentials remain failures;
malformed records do not become ordinary EOF. All externally visible errors stay
static: handshake request/response objects, credentials and clipboard data are
never formatted into diagnostics.

Regression coverage includes cancelled/expired setup followed by successful
pairing, wrong CA/token/peer rejection, bidirectional pinned TLS, duplicate relay
slots, full-duplex transfers and malformed-record classification. These tests run
in isolated containers where workstation policy prohibits listeners.

The [relay pilot reconnect option](OUTBOUND-PILOT.md) builds on this primitive.
It retains the logical signing session, clock/order state and original TTL and
validates the same peer before using a replacement transport.
