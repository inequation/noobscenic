# JSON Schemas — Proscenic M7 Pro protocol

Draft 2020-12 schemas for every JSON-based API surface. **Transport, framing and
crypto cannot be expressed in JSON Schema** — those are documented in `../PROTOCOL.md`
and summarized here.

| File | Channel | Transport | Encoding |
|---|---|---|---|
| `channelA_rest.schema.json` | Cloud REST | HTTP/HTTPS (base `https://mobile.proscenic.cn/`) | **requests: form-urlencoded** (modeled as field-set objects); responses: JSON `{code,message,data}` |
| `channelB_gateway.schema.json` | Push gateway | persistent raw **TCP** (getSockAddr ip:port) | JSON text + **`#\t#` (0x23 09 23)** delimiter per frame; **device→cloud `data` = plaintext JSON**; **cloud→device command `data` = `base64(AES-128-ECB(json, session[:16]))`** |
| `channelC_local.schema.json` | Local provisioning/control | **UDP** JSON datagrams (soft-AP `LDRobot`, robot 192.168.78.1; LAN port `rand()%1000+9000`) | one JSON object per datagram |

Notes for the implementer:
* The `$defs` in A and B are keyed by endpoint / `infoType`; validate the envelope,
  branch on `infoType` (B) or path (A), then validate against the matching `$def`.
* **Register returns `data.session` + `data.cookies`** (NOT a field named `token`).
  AES key = **`session[:16]`**; `cookies` is the sid the device echoes as
  `Cookie: cookies=<value>` on every Channel-A request. Return `code:102` to force a
  re-register. A server minting a `token` field instead establishes no key and the
  robot re-registers forever.
* **Cloud→device commands MUST include an integer `encrypt` field** (`0` = plaintext
  `data` object, no AES; `1` = `data` is `base64(AES-128-ECB(json, session[:16]))`).
  A command **without** `encrypt` is silently dropped by the device — use `encrypt:0`
  to skip AES entirely. Device→cloud is always plaintext (the device never encrypts).
* **Heartbeat is mandatory:** answer every Channel-B `21006` Ping. A safe pong is
  `{"infoType":21006,"data":{}}` (receipt marks the device online); optionally add
  `"isExistConnect":true`. The server must also `#\t#`-terminate the frames it sends.
* `infoType` (wire) and `EID_*` (in-process bus, see `../eid_catalog.tsv`) are
  **separate number spaces**.
* Soft spots to confirm against a live capture: the exact **pong** reply to `21006`,
  the AES-ECB **pad byte** for cloud→device commands (prefer space `0x20`), the
  `setSta` request keys, and map cell-value semantics.
