# Architecture

DeskBridge uses native shells around a Rust core.

```text
macOS Swift/AppKit shell       Windows WPF admin shell
          |                               |
          | process control/config         | service control/config
          v                               v
      deskbridge client  <----TCP---->  deskbridge server
          |                               |
          v                               v
   input injection backend         input capture backend
```

The Rust core owns the protocol, config schema, layout mapping, health model, and
input event types. Native shells should be thin supervisors around that core.

## Why Not WebView First

The primary product risk is not a complicated UI. It is service lifecycle, local network diagnosis, permissions, foreground/background behavior, and native trust. Those are native platform concerns.

The UI should therefore stay native and narrow:

- macOS: menu bar, small settings window, permission health, connection status.
- Windows: clear server panel, layout grid, firewall/service status.

## Protocol

The current protocol is length-prefixed JSON:

1. Client connects to `server:24800`.
2. Client sends `hello`.
3. Server validates protocol version and screen name.
4. Server sends `welcome`.
5. Peers exchange `ping`/`pong`.
6. Server sends `input` packets.
7. Client applies packets and replies with `ack`.

This is intentionally inspectable during early product work. A future protocol can switch the frame payload to MessagePack or protobuf without changing the transport state machine.

## Configuration

The JSON config has four stable top-level areas:

- `server`: server screen name and listen address.
- `client`: client screen name and target server address.
- `layout`: named screens plus directed physical edge links.
- `reliability`: heartbeat and reconnect timing.

The daemon accepts `--config` for `server`, `client`, and `diag`; command-line flags remain useful for quick one-off tests.

## Reliability Model

The client must distinguish these states:

- `port closed`: server not started, firewall, or machine asleep.
- `tcp open but handshake rejected`: wrong screen name, protocol mismatch, trust error.
- `connected but stale`: heartbeat timeout.
- `connected`: active session with recent rx.

The UI should show the state and the next actionable fix.

The macOS shell supervises the client process and restarts it when the user has enabled auto reconnect. The Rust client also keeps retrying inside one process, so the product has two recovery layers: protocol-level reconnect and app-level daemon restart.

## Server Capture Path

The server separates capture from transport:

1. A platform capture source emits `LocalPointer` and input events.
2. The Rust router checks the configured layout edge links.
3. When the pointer hits a linked edge, the active screen switches to the target client.
4. While a client screen is active, keyboard, mouse button, wheel, and movement events are sent as protocol `input` packets.

On Windows, `deskbridge server --capture` installs low-level mouse and keyboard hooks. On macOS, it installs a listen-only CoreGraphics event tap and requires Accessibility plus Input Monitoring permissions. `--demo-events` remains the portable loopback verifier when platform capture is not available.

## Debug Control Path

Diagnostic sessions can send `DebugRequest` messages to the server. The server forwards those requests over the already-connected client session and returns the client's `DebugResponse` to the diagnostic caller.

The first supported commands are:

- `display-info`: read the target client's display size and current pointer location.
- `move-mouse`: run a target-side mouse injection test through the same input sink used for normal remote control.
- `logs`: return recent target-side debug entries kept in the active client session.

This keeps the Mac client outbound-only: debug operations do not require opening a listener on the Mac.

## Clean-Room Note

Input Leap is useful prior art, but DeskBridge should not copy its implementation if we want license flexibility. The current code only implements behavior from first principles.
