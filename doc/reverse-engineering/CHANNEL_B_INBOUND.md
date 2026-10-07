# Channel B — inbound (cloud→device) message envelope — **corrected 2026-10-07**

**Why this document exists.** A replacement server sent flat frames of the form
`{"infoType":N,"encrypt":0,"data":{…}}` and observed: TCP link healthy (pongs answered),
but **zero command execution and zero `cleanPack/response` POSTs** for every infoType tried
(20001, 21005, 21020, 21024, 30000). Re-tracing `network_proxy` shows the robot does **not**
dispatch the frame it receives — it dispatches **the contents of the frame's `data` member**,
and *that object* must itself carry the integer `infoType` plus its own `data`/`dInfo`.
Flat frames are dropped at the dispatcher with a log-only `WTF!!! Recv Json is %s`, *after*
the link-liveness timer has already been refreshed — so from the wire they look like silence.
This supersedes the envelope shown in earlier revisions of `PROTOCOL.md` §B,
`FUNC_STATUS.md` §2.2/§2.3, `FUNC_COMMANDS.md` examples and the schema.

## The two inbound envelopes (disassembly-verified)

Wire (Channel B, `#\t#`-framed):

```
{"encrypt":0,"data":<MESSAGE>}
{"encrypt":1,"data":"<base64( AES-128-ECB( jsoncpp_text(<MESSAGE>), session[:16] ) )>"}
```

`<MESSAGE>` (the object the dispatcher actually processes):

```json
{"infoType":<int>, "data":<handler payload>, "dInfo":{"ts":"<string>","userId":"<string>"}}
```

* **`encrypt`** (integer) and **`data`** (member) are required at the **outer** level — this is
  the hard gate in `FUN_00457f08`. No `encrypt` ⇒ silent drop.
* `<MESSAGE>` has the **same shape the robot itself sends** (top-level `infoType` + `data`),
  plus **`dInfo`**, the reply-correlation object.
* **`dInfo` is mandatory for every command whose handler replies** — the reply builder
  requires **string** members `dInfo.ts` and `dInfo.userId`; otherwise it logs
  `Check Your Code, the dInfoJson is %s, retJson is %s` and **does not POST**. Include it
  always; use any non-empty strings (the robot echoes them back in the reply).
* For `encrypt:1`, the AES plaintext is the **jsoncpp text of `<MESSAGE>`** — not of the whole
  frame. (Padding disabled; pad to a 16-byte multiple with spaces or don't use `encrypt:1`.)

### Examples

```json
{"encrypt":0,"data":{"infoType":21006,"data":{"isExistConnect":true}}}
{"encrypt":0,"data":{"infoType":21024,"data":{"cmd":"setledswitch","value":0},"dInfo":{"ts":"1759...","userId":"probe"}}}
{"encrypt":0,"data":{"infoType":20001,"data":{},"dInfo":{"ts":"1759...","userId":"probe"}}}
{"encrypt":1,"data":"<base64 of the AES of {"infoType":21005,"data":{"mode":"smartClean"},"dInfo":{...}}>"}
```

## Evidence chain (LS_S6 0.7.1, `network_proxy`, load base 0x400000)

1. **Framing.** `TcpProtocolJson` per-byte state machine (vtable `0x4947b8`, framer
   `FUN_0046d258`): accumulates only after `{`, splits on `#\t#`, strips the delimiter,
   NUL-terminates, and invokes the registered callback per complete frame, unconditionally.
   Callback = `FUN_00457f08` (registered in `FUN_00458790`: handle `+0x20`, owner `+0x28`).
2. **Callback top.** `FUN_00457f08`'s **first** statement is `FUN_00454928(heartbeat,0)` —
   it refreshes the heartbeat's last-activity tick (`+0x30`) **before parsing or any gate**.
   The reconnect watchdog (`FUN_00455090`) tears the link down when `now > last + 15 s`
   (checked 1×/s). **Consequence:** any complete frame — accepted or silently dropped —
   keeps the link alive. *A healthy link proves frames are read, not that anything dispatches.*
3. **Outer gate** (silent on failure): `msg["encrypt"].isInt()` (key `0x490950`) **and**
   `msg.isMember("data")` (key `0x488658`).
4. **Forward.** Both encrypt branches converge and forward **only `toStyledString(msg["data"])`**
   (or, for `encrypt:1`, the parsed decryption of it) to the virtual method
   `ProtocolLd::OnNewData` (`FUN_00471208`, vtable slot `+0x10` of `ProtocolLd` at
   interface `+0x100`). Verified in disassembly: the styled string's source is the Value
   slot assigned from `msg["data"]` (`0x458058–0x458064`), and the outer message is never
   forwarded. **The outer envelope is discarded here.**
5. **Dispatcher** `FUN_00470b28`: requires `isObject` **and** `member "infoType"` (key
   `0x488648`) **as an integer**, then reads its `"data"` (key `0x488658`) and `"dInfo"`
   (key `0x490a48`). Missing/invalid ⇒ `WTF!!! Recv Json is %s` (`protocol_ld.cpp:0x39`).
   Unknown infoType ⇒ `Error : Unknown infoType %d` (`:0x43`). All of 20001, 21001–21006,
   21010–21012, 21014–21031, 30000 are registered — a *correctly enveloped* frame cannot
   miss the table.
6. **Reply path.** Handler reply callback → `FUN_00459100`: **requires `dInfo["ts"].isString()`
   and `dInfo["userId"].isString()`** (keys `0x48e0d0`, `0x490a08`) or it refuses to queue the
   POST (`Check Your Code…`). On success: `FUN_0045ed80` → FIFO → worker `FUN_0045ee18`
   curl-POSTs `sn&infoType&ts&userId&data` to the cloud singleton URL member `+0x68`
   (= `cleanPack/response`).
7. **Pong.** The `21006` handler `FUN_00457b30` calls `RecvPong` unconditionally
   (marks device online), then reads `isExistConnect` (key `0x490910`) **as a bool from its
   payload object** — i.e. from `<MESSAGE>.data`. So the correct pong is
   `{"encrypt":0,"data":{"infoType":21006,"data":{"isExistConnect":true}}}`. A flat pong
   refreshes the link but never reaches `RecvPong` — so the app-online flag stays unset and
   no status/map pushes are enabled.

## Why the original frames behaved exactly as observed

* `{"infoType":20001,"encrypt":0,"data":{}}` → passes the outer gate; the forwarded object is
  `{}`; dispatcher: no integer `infoType` ⇒ `WTF!!!` ⇒ return. No handler, no reply, no action.
* Same for every other infoType, and for the pong: `{"isExistConnect":true}` has no `infoType`.
* The link stayed up throughout because step 2 precedes everything.
* No HTTP appeared because nothing reached the reply path — and even a dispatched command with
  a bad `dInfo` would refuse to reply.

## Live confirmation matrix (what to send and what it proves)

| # | Send | Expect | Proves |
|---|---|---|---|
| 1 | `{"encrypt":0,"data":{"infoType":20001,"data":{},"dInfo":{"ts":"<now-ms-as-string>","userId":"probe"}}}` | HTTP `POST cleanPack/response` with a status record | envelope + dInfo correct |
| 2 | #1 without `dInfo` | no HTTP (dispatch still happens; reply refused) | isolates the dInfo requirement |
| 3 | `{"encrypt":0,"data":{"infoType":21024,"data":{"cmd":"setledswitch","value":0}}}` | LED changes | handler execution without reply context |
| 4 | `{"encrypt":0,"data":{"infoType":21006,"data":{"isExistConnect":true}}}` | status pushes begin (`uploadEvents` POSTs on state change); `RecvPong`/`on new ping pong msg...` if a device log is being pulled | pong path + app-online flag |
| 5 | old flat frame | nothing but a live link | control for the failure mode |

If any corrected variant still fails, pull the device log (`{"req":"getLog"}` on UDP 7913 —
note the listener only runs when the robot boots in AP mode; see `PAIRING_LOG_ANALYSIS.md`) and
grep for `recv : %s`, `WTF!!! Recv Json is %s`, `Parse into json error`, `Error : Unknown
infoType`, `decrypt data failed. try register again!`, `Check Your Code`, `Push Response Frame`.

## Uncertainty

* No vendor-side capture exists; the shapes above are inferred from the parser, dispatcher and
  reply-builder code (all cited claims disassembly-verified). The test matrix exists to confirm.
* Content semantics of `ts`/`userId` (e.g. whether `userId` must match the bound account) are
  not validated; the robot appears to only echo them.
* `dInfo` presence is *required* for replying commands; for non-replying commands (21020, 21005
  without a fail-reply…) its absence is not expected to matter, but include it anyway.
