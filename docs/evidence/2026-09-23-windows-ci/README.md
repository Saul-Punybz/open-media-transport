# Windows CI: socket shutdown does not wake blocked reads

23 Sep 2026, GitHub Actions `ubuntu-latest`, `macos-latest`, `windows-latest`,
branch `fix/windows-ci` (draft PR #1).

## What failed

After the M12 prerequisites were merged, the Windows job failed
`receiver::tests::drop_is_bounded_with_a_stalled_sender` (the drop took about
4 s, the bound is `DROP_TIMEOUT` + 1 s = 3 s), and
`discovery_server::tests::clients_find_each_other_through_the_server` ran for
over 15 minutes until the run was cancelled. The same code passed on Linux
and macOS.

## What was measured

`socket_diag.rs` is a throwaway integration test (not part of the crate; copy
it to `crates/open-media-transport/tests/` to run it with
`cargo test --test socket_diag -- --nocapture --test-threads=1`). It was run on
all three runners in CI runs 35880930894 and 35881670113;
`socket-diag-output.txt` is its output.

| Probe | Linux | macOS | Windows |
|---|---|---|---|
| Read blocked on a `try_clone` handle, `shutdown(Both)` on the other | `Ok(0)` at once | `Ok(0)` at once | still blocked after 3 s |
| Same with `shutdown(Read)` | `Ok(0)` at once | `Ok(0)` at once | still blocked after 3 s |
| Write blocked on a full send buffer, `shutdown(Both)` on the other handle | `BrokenPipe` at once | `BrokenPipe` at once | still blocked after 3 s |
| Blocked read after the *peer* shuts down | `Ok(0)` | `Ok(0)` | `Ok(0)` |
| `connect` to a closed local port | refused in < 1 ms | refused in < 1 ms | refused after 2.0 s |
| `bind 127.0.0.1:P` while `[::]:P` (dual-stack) is bound | `AddrInUse` | allowed; connects go to the 127.0.0.1 one | allowed; connects go to the 127.0.0.1 one |
| Ephemeral port collisions, v4 `127.0.0.1:0` vs dual-stack `[::]:0`, both orders | 0 / 3000, 0 / 500 | 0 / 3000, 0 / 500 | 0 / 3000, 0 / 500 |

## What it means

Every connection in the crate has a thread blocked in `read`, stopped by
`shutdown` from the thread that closes the connection. On Windows that
thread stays blocked until the peer sends or closes. So:

- `Receiver::drop` with a silent sender waited `DROP_TIMEOUT` once per
  connection (video, then audio): about 4 s. The reader threads leaked.
- `discovery_server::Client::drop` and `Server::drop` joined their reader
  (and accept) threads without a bound, so they depended on the other end
  answering the FIN. A peer that does not answer blocks the drop forever.
  Those joins are the only unbounded waits in the hung test; which one hung
  in that run was not reproduced: the test passed alone and in the full
  suite on every later run, and the last two rows of the table rule out
  another test's listener taking the server's port.

## Fix

`src/net.rs`: on Windows, reader sockets get a 100 ms read timeout and
`net::read` returns `Ok(0)` once the connection's stop flag is set; on
Linux and macOS both are no-ops. Receiver, sender and discovery-server
readers use it; the discovery client and server drops wait at most
`DROP_TIMEOUT`; `Receiver::drop` shuts both connections before waiting,
under one deadline. The CI test step has `timeout-minutes: 15`.

## Not fixed

A sender's writer thread blocked on a receiver that stopped reading is not
woken by `shutdown` on Windows either (row 3). `Sender::drop` is still
bounded (it stops waiting after `DROP_TIMEOUT`), but that thread and its
socket live on until the receiver reads or disconnects. A write timeout
would end it, but on Windows a timed-out blocking send leaves the stream
in an undefined state, so it is not used for media.
