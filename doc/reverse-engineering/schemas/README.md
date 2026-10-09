# JSON Schemas — Proscenic M7 Pro protocol

Draft 2020-12 schemas for every JSON-based API surface. **Transport, framing and
crypto cannot be expressed in JSON Schema** — those are documented in `../PROTOCOL.md`
and summarized here.

| File | Channel | Transport | Encoding |
|---|---|---|---|
| `channelA_rest.schema.json` | Cloud REST | HTTP/HTTPS (base `https://mobile.proscenic.cn/`) | **requests: form-urlencoded** (modeled as field-set objects); responses: JSON `{code,message,data}` |
| `channelB_gateway.schema.json` | Push gateway | persistent raw **TCP** (getSockAddr ip:port) | JSON text + **`#\t#` (0x23 09 23)** delimiter per frame; **device→cloud `data` = plaintext JSON**; **cloud→device frame = `{"encrypt":0|1,"data":<MESSAGE>}`** (`../CHANNEL_B_INBOUND.md`) |
| `channelC_local.schema.json` | Local provisioning/control | **UDP** JSON datagrams (soft-AP `LDRobot`, robot 192.168.78.1; LAN port `rand()%1000+9000`) | one JSON object per datagram |

Notes for the implementer:
* The `$defs` in A and B are keyed by endpoint / `infoType`; validate the envelope,
  branch on `infoType` (B) or path (A), then validate against the matching `$def`.
* **Register returns `data.session` + `data.cookies`** (NOT a field named `token`).
  AES key = **`session[:16]`**; `cookies` is the sid the device echoes as
  `Cookie: cookies=<value>` on every Channel-A request. Return `code:102` to force a
  re-register. A server minting a `token` field instead establishes no key and the
  robot re-registers forever.
* **Cloud→device commands use a doubly-nested envelope** (corrected 2026-10-07,
  `../CHANNEL_B_INBOUND.md`): the frame is `{"encrypt":0|1,"data":<MESSAGE>}` with
  `MESSAGE = {"infoType":N,"data":{…},"dInfo":{"ts":"<str>","userId":"<str>"}}` — the
  device dispatches the **contents of the outer `data`**. `encrypt` is mandatory and must be
  an integer (`0` = plaintext `MESSAGE` object; `1` = `data` = `base64(AES-128-ECB(
  jsoncpp(MESSAGE), session[:16]))`). A command without `encrypt` is silently dropped; a
  `MESSAGE` without an integer `infoType` dies one step later (log-only `WTF!!!`).
  `dInfo.ts`/`dInfo.userId` must be **strings** for any command whose handler replies.
  Device→cloud is always plaintext (the device never encrypts).
* **Heartbeat is mandatory:** answer every Channel-B `21006` Ping **with the envelope**:
  `{"encrypt":0,"data":{"infoType":21006,"data":{}}}` — receipt marks the device online;
  put `"isExistConnect":true` inside the inner `data` to set the app-online flag
  (status pushes). A flat pong refreshes the link timer but never reaches the handler.
  The server must also `#\t#`-terminate the frames it sends.
* `infoType` (wire) and `EID_*` (in-process bus, see `../eid_catalog.tsv`) are
  **separate number spaces**.
* Soft spots to confirm against a live capture: the AES-ECB **pad byte** for cloud→device
  commands (prefer space `0x20`), the `setSta` request keys, and map cell-value semantics.
