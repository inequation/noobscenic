# Proscenic M7 Pro — Device ↔ Cloud & Local Control Protocol

Reconstructed from `network_proxy` (`src/interface/product/ld/*`, `src/local/*`,
`src/component/https_base`), `libcpc.so`, the boot/provisioning scripts, and live
probing of the vendor cloud. **[proven]** = executed/verified, **[static]** =
from decompiled code, **[live]** = confirmed against the running server.

> **⚠ Firmware-variant caveat (read `FIELD_NOTES.md`).** This is reverse-engineered
> from the **2020 `LS_S6` firmware sample** (v0.7.1). A physical unit tested in the
> field runs **different firmware** (SSID `Proscenic-6716_…`, model `6716`). It **does**
> expose Channel C, but on a **fixed UDP port `7913`** — not the `LS_S6` sample's
> randomized `9000–9999` range. **Correction (owner report):** an earlier field sweep
> covered only `9000–9407` and saw those closed, wrongly concluding Channel C was
> absent — it had simply scanned the wrong port. Channel C is live on **UDP `7913`** on
> the real M7 Pro unit. Channels A/B are untested on that unit. Re-verify the exact
> port/SSID against your target unit.

There are **three** separate channels. Do not conflate them.

```
                 ┌────────────────────────────────────────────────────────┐
   APP  ─────────┤ (A) Cloud REST     HTTP/HTTPS, form-urlencoded → JSON    │
                 │ (B) Push gateway   raw TCP, #\t#-delimited JSON (AES cmds)│──── device
   APP (LAN/AP) ─┤ (C) Local config   UDP JSON datagrams {"cmd":...}        │
                 └────────────────────────────────────────────────────────┘
```

---

## (A) Cloud REST API — device ⇄ cloud

* **Style:** classic REST-ish HTTP. **Both HTTP and HTTPS** are used. The control
  API base defaults to **`https://mobile.proscenic.cn/`** (region variant
  `…com.de`), stored in `/data/bin/Run/Config/url`; large binary fetches (firmware,
  maps) come from an Alibaba **OSS/openresty** host over **plain HTTP**
  (`http://47.254.154.181/…`). The robot does **no TLS validation** (RE-repo
  Finding 3). **[static/live]**
* **Request encoding:** `application/x-www-form-urlencoded` bodies (a few GETs use
  query strings). **Response encoding:** JSON. **[static]**
* **Envelope:** responses are `{"code":<int>,"message":"<str>","data":{…}}`.
  `code:0` = success; **`code:102`** = *sid/session expired* → device **re-registers**;
  `code:212` = auth fail. (verified in every reporter, e.g. `get_sync.cpp`.) **[static/live]**
* **Session cookie:** after `register`, the device sends its sid on **every** Channel-A
  request as the HTTP header **`Cookie: cookies=<data.cookies value>`**
  (`CURLOPT_COOKIE`). Your server issues that value in the register response’s
  `data.cookies` and treats the returned cookie as the sid. **[proven]**
* **`data=` is plaintext JSON** (not base64/AES, and not always URL-escaped) — see
  Channel B "Payload crypto". **[proven]**

### Endpoints (all under the base, e.g. `POST https://mobile.proscenic.cn/cleanPack/...`)
| Endpoint | Method | Purpose | Body / notable fields |
|---|---|---|---|
| `cleanPack/register` | POST | device registration → issues **session + cookie** | `sn=<sn>&ld_sn=<ld_sn>&sig=<urlenc b64>&ts=0` → resp `data.{session,cookies}` |
| `cleanPack/binding` / `cleanPack/unbinding` | POST | bind/unbind device to a user account | **observed body (this unit):** `sn=<sn>&ts=<sn>&userId=<userId>` — the `ts` field repeats the **SN** (vendor quirk, don't validate it) — resp `{"code":0}` = bind success |
| `cleanPack/getSockAddr` | POST | get the **push-gateway** `ip:port` (channel B) | **cookie-authenticated**, runs after `register`; request body fields not fully pinned (⚠ gap — see note); resp `data.addr_list[]={ip,port}`, device iterates the list |
| `cleanPack/sync` | POST | device-attribute / version sync (OTA check) | see below |
| `cleanPack/response` | POST | device → cloud command ACK/results | infoType payloads |
| `cleanPack/uploadEvents` | POST | telemetry incl. **home maps/paths** (LZ4) | `devType=3&sn=%s&taskid=%llu&event=%d&createtime=%d&data=%s` |
| `cleanPack/uploadLogs`, `uploadStats`, `uploadSingle` | POST | logs / statistics | |
| version check | GET | `…?version=1&sn=<sn>&companyId=<id>` → `{code,hasUpdateFile,downUrl,fullversion,md5,…}` | |

**Channel-A POST body templates vary by endpoint** (all form-urlencoded; `data` =
plaintext JSON) **[proven]**:
```
sn=%s&ts=%llu&data=%s                                            (generic report)
sn=%s&ts=%lu&data=%s
sn=%s&infoType=%d&ts=%s&userId=%s&data=<json>                    (user-scoped report)
devType=3&sn=%s&taskid=%llu&event=%d&createtime=%d&data=%s       (uploadEvents)
devType=3&sn=%s&ts=%s&qid=%s
```
A server that only accepts `sn&ts&data` will reject some uploads — accept the extra
fields (`devType`,`taskid`,`event`,`createtime`,`infoType`,`userId`,`qid`).

**`cleanPack/sync` body** (the device-attribute/version report) **[static]**:
```
sn=<serial>&companyId=<int>&mcuVer=<h185v60>&version=<0.7.1>&versionCode=<1241>
   &gitSha=<hex>&cloud=psnk            (a &uuid=<...> variant also exists)
```
`versionCode` = `GetGitCnt()` (compiled-in, 1241 for this build). Live test:
`POST https://mobile.proscenic.cn/cleanPack/sync` currently returns
`{"code":102,"message":"你的sid过期啦","data":{}}` — i.e. it needs a valid `sid`
first obtained via `register`/login. **[live]**

**Observed on the wire (robot → our server, 2026-10-05 19:56:14Z, 98 B) [proven]:**
```
sn=LSLDSM7PRO20403551&companyId=48&mcuVer=S6&version=0.7.1&versionCode=1241&gitSha=NULL&cloud=psnk
```
i.e. for this unit `companyId=48` (Proscenic), `mcuVer=S6` (the LS_S6 base),
`version 0.7.1`, `versionCode 1241`, `gitSha=NULL`, `cloud=psnk` — all matching the
analyzed image. Sent once right after the link came up, answered `{"code":0}`.

**Firmware download URL** is a *signed* OSS request **[static]**:
```
<oss>/robotDrivers/<ms-timestamp>?itemId=5&clientId=11&fileTypeId=<n>
   &fileName=<name>&from=mpc_clean_firmware&time=<t>&_taskid=<tid>&_sign=<sig>
```
The downloaded file is the RSA-signed container from `REPORT.md` §1.

### Bootstrap order, binding, and two open gaps a server author must close
* **Order:** `register` (get `session`+`cookies`) → `getSockAddr` (get gateway
  `ip:port`, sent with the cookie) → open the Channel-B TCP socket → `10001`
  handshake → steady state. `getSockAddr` runs through the same cookie-setting curl
  path as the reporters, so the cookie exists by then.

> **✅ LIVE cloud probe — 2026-09-22 [live]** against the reachable device server
> `mobile.proscenic.cn` (120.78.28.78). (The app's own REST host `bl-app.robotbona.com`
> has no A record / is unreachable; `mobile.proscenic.cn` is the firmware's
> `cleanPack/*` device server and confirms the Channel-B bootstrap end to end.)
> - `POST /cleanPack/register` body `sn=<sn>&ld_sn=<sn>&sig=x&ts=0` → **200**
>   `{"code":0,"message":"成功","data":{"bindStatus":0,"session":"<16 chars>","cookies":"<32 chars>"}}`.
>   The server accepted a **fake SN and `sig=x`** — so **`sig`/RSA is NOT validated**
>   (confirms the "sig can be a no-op" note); `session` is exactly **16 chars** (= the
>   AES-128 key directly); `cookies` is 32 chars (the sid).
> - `GET /cleanPack/getSockAddr?version=1&sn=<sn>&companyId=<n>` with header
>   `Cookie: cookies=<cookies>` → **200**
>   `{"code":0,"message":"success","data":{"addr_list":[{"ip":"47.107.125.40","port":4430}]}}`.
>   So **Gap 1 is closed**: `getSockAddr` is a **GET** with query `version/sn/companyId`,
>   cookie-authenticated; `companyId` does not affect the address. The **real production
>   push-gateway (Channel B) is `47.107.125.40:4430`** (Alibaba Cloud, Shenzhen). A
>   rehome replaces this with your own ip:port (via `setUrl` ip+port → `ip_port.json`,
>   or by serving your own `getSockAddr`).
> - `POST /cleanPack/sync` with a minimal body → 400 (needs the real field set/sig);
>   not required for rehome.
> All probes used an obviously-fake serial (`RE_PROBE_…`, `bindStatus:0`, unbound) — no
> account/device state was changed.
* **⚠ Gap 1 (was: `getSockAddr` request body) — RESOLVED** by the live probe above:
  `GET`, query `version=1&sn=<sn>&companyId=<n>`, `Cookie: cookies=<cookies>`; response
  `data.addr_list[]={ip,port}` (device iterates the list). Note the on-wire response key
  is **`addr_list`** (a list), whereas the local `/data/bin/Run/Config/ip_port.json` file
  the device derives from it uses flat **`{"ip","port"}`**.
* **Gap 2 — binding state machine (`bind_user.cpp`) — now CONFIRMED end-to-end
  [proven, first live capture 2026-10-05].** The `BindUser` flow (`StartBind`,
  `need to update bind, BDS_WAIT_CONN -> BDS_SEND_BIND..`, `send preBind msg
  success..`, `directly bind success..`, `check bind status timeOut..`, `Bind
  Timeout`, `Recv unexpect SetBindSuccess funccall`) POSTs its preBind to
  **`cleanPack/binding`** (confirmed — was "likely"). The moment the device got a
  working Channel-B session (TCP + `10001` + `21006` pongs), it fired **three Channel-A
  requests within ~100 ms, on three separate connections** — all with the SN above and
  all answered by a plain catch-all `{"code":0,"data":{}}`:

  | # | endpoint | body (observed) | answer that mattered |
  |---|---|---|---|
  | 1 | `POST /cleanPack/binding` | `sn=LSLDSM7PRO20403551&ts=LSLDSM7PRO20403551&userId=Foo` (54 B) | `code:0` → **"directly bind success"** → EID `0x460` → **pairing indication ended** |
  | 2 | `POST /cleanPack/register` | `sn=…&ld_sn=14411442DB220CBE&sig=<urlenc b64 RSA>&ts=0` (152 B) | `code:0` accepted, **no session/cookies issued** (see below) |
  | 3 | `POST /cleanPack/sync` | see §A sync above (98 B) | `code:0` |

  Two consequences:
  * **A `code:0` catch-all is sufficient to complete the bind** — for a bind-only
    rehome the server needs no real logic; the preBind's `code:0` is the whole trigger.
  * Because no `session`/`cookies` were returned (and because `register`'s `ts` is a
    literal `0`, `sig` covers `<SN>:<unix_time>`), the device sent **no `Cookie:`
    header at all** — cookieed requests (`getSockAddr`, reporters) only start once a
    register response actually carries `data.cookies`. And with an empty session there
    is **no Channel-B AES key** → cloud→device commands must use `encrypt:0`.
  * Still open: whether map upload / command execution is *gated* on bind success —
    the bind here completed via catch-all, so the next capture (with real handlers)
    settles it.

### App ⇄ cloud (from the RE-repo README, for completeness) **[static]**
`POST /user/login` (JSON), `GET /user/getEquips/<user>`,
`POST /instructions/<sn>/<cmdCode>` (relay a command to the device, e.g. cmdCode
`21012` charge start), `POST /appInit/getSockAddr`. This is how the *app* drives the
device via the cloud; the device side is channels A/B.

---

## (B) Push gateway — raw TCP, framed JSON (+AES)

The realtime control/telemetry channel. All function refs below are in
`network_proxy` (`tcp_handle.cpp`, `response_handle.cpp`, `get_register.cpp`) and
were verified by decompilation unless marked otherwise.

### Transport & framing **[proven]**
* Persistent **raw TCP** socket to the `ip:port` returned by `getSockAddr`
  (persisted in `/data/bin/Run/Config/ip_port.json`). Not HTTP.
* **Frame = `<UTF-8 JSON text>` followed by the 3-byte delimiter `#\t#` = `23 09 23`.**
  Verified in **both directions**: the sender (`FUN_0046c950`, `persistent_connection.cpp`)
  writes the JSON then the constant at `0x494710` (bytes `23 09 23`); the inbound
  reader splits the stream on those bytes (`cmp #0x23`/`ccmp #0x9`). **The server MUST
  also terminate every frame it sends with `#\t#`** — the device's inbound splitter
  requires it (there is **no** length prefix; "length-framed" would be wrong).
* **Message object:** `{"infoType":<int>, "data":{…}, "dInfo":{…}}` — this is both the
  object the robot dispatches and the shape it emits. **Cloud→device frames wrap it in an
  envelope: `{"encrypt":<int>,"data":<message object>}`** (double nesting — see "Payload
  crypto" below and `CHANNEL_B_INBOUND.md`). Messages can also carry `"message":"ok"|"fail"`
  and `"reason":"<str>"` (on errors). (`"packId"` belongs to the separate LAN-UDP
  remote-control handler, not this channel — §C.)

### Handshake **[proven]**
On connect the device sends (then `#\t#`):
```json
{"infoType": 10001, "connectionType": 1, "data": {"token": "", "sn": "<SN>"}}
```
Note the handshake `token` field is **empty** — the AES key is *not* carried here;
it is the registration **session** key (below). `connectionType` **= 1** is the only value
emitted by this firmware (built literally in `FUN_0046c950`); no other value is
produced by the robot, so treat `1` as "primary control connection". (The
`70001`/`token` message from the RE-repo Finding 5 is **app-side only** — the string
`70001` appears in **no** robot binary, so it is not part of the robot's protocol; the
robot's AES key is the register `session`, below, not any 70001 token.)

### Payload crypto — direction matters (corrected) **[proven]**
**Encryption is asymmetric — do not assume all `data` is encrypted:**
* **device → cloud** (map `20002`, path `21011`, ping `21006`, and every Channel-A
  `data=` report): the device builds **plaintext JSON** and sends it as-is. The
  reporters (`map_send.cpp`, `msg_report.cpp` `DealNewEvent`, `msg_report_ld.cpp`
  `ReportPush`, `backup_map.cpp` `PushMessage`) `sprintf` plaintext straight into
  `data`. **A server never has to decrypt what the robot sends.**
* **cloud → device** command: the frame **MUST carry an integer `encrypt` field alongside
  `data`** — this is a hard gate — and the robot then dispatches **the contents of `data`**
  (corrected 2026-10-07; see `CHANNEL_B_INBOUND.md`). Inbound callback `FUN_00457f08`
  (`interface_obj.cpp`, the connection onMessage) does, in order (all verified):
  1. `msg["encrypt"].isInt()` — **if not an integer, the frame is dropped** (member
     string `"encrypt"` @ `0x490950`).
  2. `msg.isMember("data")` — **if `data` is absent, dropped** (`"data"` @ `0x488658`).
  3. `e = msg["encrypt"].asInt()`. **`e != 1` ⇒ `data` is taken as a plaintext JSON
     object**; **`e == 1` ⇒ `data` is a string** =
     `base64( AES-128-ECB( json, key = SESSION[0:16] ) )`, decrypted and parsed
     (`FUN_00457da0` → `FUN_004789a8`: `EVP_aes_128_ecb`, **padding disabled**
     `EVP_CIPHER_CTX_set_padding(ctx,0)`, key size `0x10` = 16 bytes; on failure:
     `"decrypt data failed. try register again!"`).
  4. **Only that `data` object is forwarded** (`toStyledString`, disasm
     `0x458058–0x458070`) to `ProtocolLd::OnNewData` (`FUN_00471208`) and the dispatcher
     `FUN_00470b28`, which require **its own integer `infoType`** plus its `data`/`dInfo`
     members. A frame whose outer `data` lacks an integer `infoType` is dropped one step
     later with a log-only `WTF!!! Recv Json is %s` — silence on the wire.

  So there are **two valid server→device command forms** — note the **double nesting**
  (`<MESSAGE>` = `{"infoType":<N>,"data":{ …handler payload… },"dInfo":{"ts":"<str>","userId":"<str>"}}`):
  ```
  {"encrypt":0,"data":<MESSAGE>}                                       # no crypto needed
  {"encrypt":1,"data":"<base64(AES-128-ECB(jsoncpp_text(<MESSAGE>),SESSION[:16]))>"}
  ```
  **`encrypt:0` lets a replacement server skip AES entirely** — the simplest correct
  path. A command with no `encrypt` field is silently dropped; `dInfo.ts`/`dInfo.userId`
  must be **strings** for any command whose handler replies, or the reply is built but
  never POSTed (`Check Your Code, the dInfoJson is %s, retJson is %s`).
  The device itself **never encrypts** (the ECB-encrypt wrapper `FUN_00478ac8` has
  **0 callers**); a CBC variant `FUN_00478c50` exists but is unused on this path.
  Matches the RE-repo `decryptor/` (`AES-128-ECB`; its "token" = our `session`).
* **Padding for `encrypt:1`:** decrypt requires a 16-byte-multiple ciphertext and does
  **not** strip padding — it null-terminates the buffer and hands it to jsoncpp. Prefer
  **space (`0x20`) padding** of the plaintext to a 16-byte multiple (jsoncpp tolerates
  trailing whitespace; embedded NULs before the terminator are risky). Or just use
  **`encrypt:0`** and avoid the question. Confirm the pad byte against one live command
  if you use `encrypt:1`.

### Registration & the session/cookie — where the AES key comes from (corrected) **[proven]**
On first use the device registers (`get_register.cpp`, `FUN_0044ec90`, lazy global):

1. Build the auth signature (`FUN_0044ea88`):
   `sig = urlencode( base64( RSA_public_encrypt( "<SN>:<unix_time>",
   /data/bin/sys_data/public.key, RSA_PKCS1_PADDING ) ) )`.
   (Note: the signed plaintext uses the **real** unix time even though the body’s
   `ts` literal is `0`; a server that logs `sig` must URL-decode it first.)
2. `POST <base>/cleanPack/register` with body
   `sn=<custom.sn>&ld_sn=<ld_sn>&sig=<sig>&ts=0`
   (`custom.sn`, `ld_sn` from `/data/bin/sys_data/`).
   **Observed live (152 B) [proven]:** `sn=LSLDSM7PRO20403551&ld_sn=14411442DB220CBE
   &sig=<128 chars: URL-encoded base64 RSA>&ts=0`
   — `ld_sn` is verbatim the contents of `/data/bin/sys_data/sn` (16 hex chars). When
   the server answers `code:0` but with `data:{}` (no session/cookies), the device
   accepts it and simply carries on with an empty session — **no re-register loop**
   (observed).
3. **Response `data` must contain TWO string fields** (verified: the device does
   `strncpy(store+0x00, data.session, 0x40)` and `strncpy(store+0x48, data.cookies, 0x38)`;
   log `mSession:%s\tmCookies:%s`; missing → `no session!!!`):
   * **`data.session`** — its **first 16 bytes are the Channel-B AES-128-ECB key**.
   * **`data.cookies`** — the **session id (sid)**. The device then sends it on every
     Channel-A request as an HTTP header **`Cookie: cookies=<value>`**
     (`CURLOPT_COOKIE`). This is the same `cookies=…` seen in the RE-repo capture.

A response of `{"code":0,"data":{"token":"…"}}` (the earlier, **incorrect** shape)
leaves `session` empty → no AES key, `no session!!!`, endless re-registration.

**Re-register trigger:** any Channel-A response with **`code == 102`** (also `212`) —
"sid expired" — forces the device to discard the session and re-run `register`
(`FUN_0044f268` path in every reporter). Your server returns `102` to force a refresh.

**Clean-room server:** *you* mint both fields in your `register` response —
put ≥16 bytes of key material in `data.session` (use `session[:16]` as your ECB key
for cloud→device commands) and any opaque `data.cookies` value (accept it back in the
`Cookie: cookies=` header). `sig`/RSA verification can be a **no-op** — the robot only
*sends* `sig`; it never validates your server’s identity.

### Heartbeat / keepalive — MANDATORY (was missing) **[proven]**
Channel B has a ping/pong keepalive (`heart_online_ld.cpp`, classes
`HeartOnline`/`HeartOnlineLd`). The device periodically sends **Ping
`{"infoType":21006,"data":{}}`** (`FUN_004545f0`, log `"Send Ping"`), tracks the last
pong, and if the server stays silent it **drops and reconnects**
(`"reconnect to server ...  isOpen=%d, send interval:%d"`; `RecvPong`=`FUN_00454880`
sets the online flag; pong handler `FUN_00457b30`, log `"on new ping pong msg..."`).
**A server that never answers pings will make the robot loop connect→drop→reconnect —
it will not "stay connected."** The server MUST reply to each `21006` Ping.
**Observed live (2026-10-05) [field]:** the first `21006` arrived within ~1 s of the
`10001` handshake, then one every **~6 s** (matching the ~6-7 s heartbeat-loop cadence
in `FUN_00455090`), and the link stayed up for the whole session once the server ponged.

**Pong contract (recovered statically; pong shape corrected 2026-10-07):** the pong handler
`FUN_00457b30` calls `RecvPong` (`FUN_00454880`), which **marks the device online on receipt
alone** (sets the online flag and stores `GetCurrentTickSec`). It then optionally reads a
boolean member **`isExistConnect`** (`@0x490910`) from the message payload and stores it
(app-online reflection); if absent it just logs `"on new ping pong msg..."`. The pong is
dispatched like every other message, so it must use the full envelope
(`CHANNEL_B_INBOUND.md`, double nesting included):
```
{"encrypt":0,"data":{"infoType":21006,"data":{}}}                      # marks device online
{"encrypt":0,"data":{"infoType":21006,"data":{"isExistConnect":true}}} # + app-online flag
```
**Why any reply keeps it alive (verified):** the inbound callback `FUN_00457f08`
(onMessage) calls `FUN_00454928(conn, 0)` as its **first** statement — before the
`encrypt` gate and before the infoType dispatch — and that call refreshes the same
online flag (+0xc) and last-activity tick (+0x30) that `RecvPong` writes. So **any
complete `#\t#`-terminated frame the server sends refreshes the online timer**, even a
flat `{"infoType":21006,"data":{}}` that never reaches the dedicated pong handler
`FUN_00457b30` (it dies at the dispatcher's integer-`infoType` requirement). Practical
upshot: answering every `21006` keeps the link up; to also set the **app-online** flag
(the thing that enables status pushes), send the enveloped pong above. The watchdog is
`FUN_00455090`: reconnect when `now > last + 15 s`, checked once per second.

**Timing:** the ping period and drop timeout are runtime variables
(`"…send interval:%d"`, `nanosleep` loop) with no compiled-in constant; **pong every
`21006` immediately** (within the RTT). Measure the actual interval/timeout from one
live session if you want a precise budget.

### infoType quick map (numeric ↔ meaning)
Full internal event map with numeric values is in **`eid_catalog.tsv`** (355 IDs).
Wire `infoType`s seen as JSON literals in the binaries:

| infoType | direction | meaning |
|---|---|---|
| 10001 | device→cloud | connect/login handshake (`connectionType`,`token`(empty),`sn`) |
| 20002 | device→cloud | occupancy-grid **map upload** (LZ4, **plaintext**) — see `MAP.md` |
| 21003 | cloud→device | `SetAreaTactics` (room/area/forbidden-zone config; **AES if encrypted path**) |
| 21006 | device→cloud | **keepalive Ping** `{"data":{}}` — **server MUST pong** (see Heartbeat) |
| 21011 | device→cloud | **clean-path** stream (`userId,pathID,startPos,totalPoints,posArray`) |
| 21020 | cloud→device | **remote control** — `data.ctrlCode` + optional `data.params`, **no reply** on this channel (confirm via the status `mode` echo). The `packId`/"rfctrl" ack belongs to the separate **LAN UDP** handler, not Channel B; the earlier "chunked pack transfer" reading was wrong (`FUNC_COMMANDS.md` §2.1, `FUNC_MAP.md` §3) |
| 70001 | app-side only | crypto-token message seen in the RE-repo capture; the string `70001` is **absent from every robot binary**, so it is **not part of the robot's protocol** and irrelevant to a replacement server (the robot's AES key is the register `session`, not this). |

Note: `infoType` on the wire and the internal `EID_*` bus IDs are **different number
spaces** — `EID_*` (0–7999, see `eid_catalog.tsv`) are the in-process ZeroMQ event
bus IDs (`ZmqManager::PostEvent(EventId,…)`); `infoType` are the cloud-facing message
type codes. The device maps between them in `response_handle.cpp`.

---

## (C) Local config channel — UDP JSON `{"cmd":…}`  ← this is how you re-home it

> **✔ Port resolved (2026-10-04, late): `7913` is hardcoded in this image.**
> The listener's `bind` constant in `network_proxy` is `sin_port = 0xe91e` (= 7913 in
> network byte order, `FUN_00468a70`). The `rand()%1000+9000` on `9000–9999`
> (`OpenUdpRemoteCtrl`) is a **different, cloud-triggered RemoteCtrl socket** — not this
> provisioning channel. The earlier field sweep of `9000–9407` was a wrong-port false
> negative; the live `6716` unit answers on **UDP `7913`** (verified end-to-end:
> getSn/setUrl/setSta/applyCfg/setID/getLog all served over it — a full live session is
> decoded line-by-line in `PAIRING_LOG_ANALYSIS.md`).

Used by the app on the **local network** (most importantly while the robot is in its
soft-AP: `apDemo` brings up AP at **192.168.78.1**, DHCP 192.168.78.50-150; the SSID is
`LDRobot` on the `LS_S6` sample but is vendor-branded on shipping units, e.g.
`Proscenic-6716_<serial>`). Handled by `network_proxy` (`local/local_debugger.cpp`,
`interface/.../wifi_config.cpp`, `udp_server_interface.cpp`). **[static]**

* **Transport:** **UDP datagrams** carrying a JSON object; the device replies with a
  JSON datagram. The provisioning listener binds **UDP `7913`** — hardcoded in this image
  (`FUN_00468a70`: `sin_port = 0xe91e`) — and runs only while the robot is in **AP mode**
  (started at boot when `/data/cfg/wifi_mode` == `"ap"`, or on AP-mode entry; it is *not*
  stopped when the robot later switches to STA, which is why it was still reachable on the
  LAN in the 2026-10-04 session). The `rand()%1000+9000` range
  (`OpenUdpRemoteCtrl`, `cp_function.cpp`) is a **separate cloud-triggered RemoteCtrl
  socket**, not this channel. **[static + field]** Note: a plain `{"cmd":"getID"}` probe is
  answered `{"result":"invalue cmd"}` — **`getID` is not a real command**; the
  `{"cmd":"getID","result":"ok","type":"ipfromapp"}` reply in old notes belongs to **`setID`**.
  Identify a unit with `getSn`. *(The command set and JSON schemas below are fully recovered;
  with shell access you can also write the config files directly — see “Re-home”.)*
* **Request shape:** `{"cmd":"<name>", …fields…}` → **Response:**
  `{"cmd":"<name>","result":"ok"|"fail","code":<int>, …}`; unknown → `{"result":"invalue cmd"}`.

### Command reference **[static]**
| `cmd` | Request fields | Response | Effect |
|---|---|---|---|
| `getID` | – | `{"result":"invalue cmd"}` (probed live; **not a command**) | *(the `ok/type:ipfromapp` reply is setID's; identify units with `getSn`)* |
| `getSn` | – | `{"result":"ok","sn":"<serial>"}` | read serial (from `/data/bin/sys_data/custom.sn`) |
| `getWifi` | – | `{"result":"ok","wifi_list":[ … ]}` | scan nearby APs |
| `checkPwd` | – | `{"result":"ok","code":<connState>}` | query Wi-Fi/join status |
| `getCfg` | – | `{"result":"ok", …, "staName":"…","staPwd":"…","staIp":"…","staMac":"…"}` | current STA/config |
| **`applyCfg`** | – (no fields read) | `{"result":"ok","code":1}` / `fail,-1` (fail = no `staName` stored) | **commit: replies, then runs `cleanpack_mode -m sta` with the stored creds** (`FUN_00464ed0`/`FUN_00467ae8` → `FUN_00464cb8`) |
| **`setSta`** | **`{"staName":"<ssid>","staPwd":"<8-64 char pass>"}`** | `{"result":"ok","code":2}` / `fail,-1` | **stores** the STA creds in memory only (no Wi-Fi change) — follow with `applyCfg` (see below) |
| `setAp` | ap params | `{"result":"ok"}` / `fail` | switch back to soft-AP |
| **`setUrl`** | **`{"url":"http://you/"}`** *or* **`{"ip":"1.2.3.4","port":<n>}`** | `{"result":"ok"}` | **set cloud base URL / push-gateway** |
| `resetWifi` | – | `{"result":"ok"/"fail"}` | clear Wi-Fi config |
| **`setID`** *(added 2026-10-04)* | **`{"id":"<userId>","deviceSN":"<robot SN>"}`** | `{"cmd":"getID","result":"ok","type":"ipfromapp"}` / `{"cmd":"getID","result":"fail"}` | **bind request.** `FUN_004647e0`: `deviceSN` must equal the robot's own SN (`DAT_004c5738`, same value `getSn` returns) or it fails. Then `FUN_00449158` stores `id` as the bind `userId` and the SN, sets "bind pending" and starts the `BindUser` thread (`bind_user.cpp`, see below). Note the reply says `getID`: the `getID` reply documented earlier is actually `setID`'s. |
| **`bindOk`** *(corrected 2026-10-04)* | – | none | `FUN_00463c60`: logs "Close mSockFd" and `close()`s the UDP config socket **without stopping the listener** (it doesn't set fd=-1 or clear the run flag) → every following `recvfrom` fails instantly on the stale fd and the loop spins at ~28 iter/s, logging `buf2Json Fail` + empty + `{result:invalue cmd}` per iteration (~85 `system()` forks/s, ~2.5 KB/s, indefinitely). **Do not send it** — it serves no purpose for a rehome. Mechanism + live evidence: `PAIRING_LOG_ANALYSIS.md` §5. |
| `{"req":"getLog"/"rmLog"}` | offset | log package (base64) | pull/remove logs |

### What `setSta` is — and the `applyCfg` commit (corrected 2026-10-03)
**“Set Station mode.”** Wi-Fi has two roles: **AP** (the robot *is* the access point,
used for pairing) and **STA/station** (the robot is a *client* that joins your router).
**Pairing is two commands, not one** **[static]**:

1. **`setSta`** `{"cmd":"setSta","staName":"<ssid>","staPwd":"<password>"}` — handler
   `FUN_00464498` (dup `FUN_00467570`, `wifi_config.cpp`). It reads the keys
   **`staName`** and **`staPwd`** (string literals at `0x4644e8`/`0x464648`; *not*
   `ssid`), `strncpy`s them into the config object (`+0x80` SSID, `+0xc0` password,
   64 B each) and replies `{"cmd":"setSta","result":"ok","code":2}`. **It does nothing
   else** — no Wi-Fi change. `getCfg` will now echo the new `staName`/`staPwd`, which
   makes it look as if pairing succeeded while the robot stays on its own AP.
2. **`applyCfg`** `{"cmd":"applyCfg"}` — handler `FUN_00464ed0` (dup `FUN_00467ae8`).
   If no SSID is stored it replies `{"cmd":"applyCfg","result":"fail","code":-1}`
   ("ApplyCfg Has Some Error"). Otherwise it **sends** `{"cmd":"applyCfg","result":"ok","code":1}`
   first (explicit `sendto`, because the AP is about to go away) and then calls
   `FUN_00464cb8` ("Config wifi begin"), which checks the password is empty or 8–64
   chars, shell-escapes both values and runs
   ```
   sh /data/bin/cleanpack_mode -m sta -s "<ssid>" -p "<password>"
   ```
   — this writes `wpa_supplicant.conf`, switches `wifi_mode`→`sta`, tears down the AP
   and associates to your router. A too-short/too-long password fails *silently* here
   (event `0xfce` only, after the `ok` reply).

Earlier revisions of this file attributed the `cleanpack_mode` call to `setSta` and
named its SSID key `ssid`; both were wrong. Expect the `applyCfg` reply to be the last
datagram you receive from the AP.

*Field caveat (updated 2026-10-04):* this is the `LS_S6` 2020 sample, but the shipping
`6716` unit runs the same build and answers the same `{"cmd":…}` set on UDP `7913`. Its
2026-10-03 attempt showed the "stored creds, stayed on the AP" symptom of a missing
`applyCfg`; the 2026-10-04 session sent `applyCfg` and the robot successfully left its AP
and joined the owner's Wi-Fi — so the recipe below is confirmed on real hardware
(`PAIRING_LOG_ANALYSIS.md` §2).

### Bind state machine (`bind_user.cpp`) — added 2026-10-04 **[static]**
`setID` only *arms* the bind; the `BindUser` thread (`FUN_004495d8`, 1 s tick) does the rest:
* **`BDS_WAIT_CONN` (0):** waits until "bind pending" is set **and** the cloud link is up
  (a flag in the cloud object, `(*(+0xa8)+0x20)+8`); then logs
  "need to update bind, BDS_WAIT_CONN -> BDS_SEND_BIND..".
* **`BDS_SEND_BIND` (1):** `FUN_004492c0` POSTs `sn=<SN>&ts=<field +0x10>&userId=<field +0x50>`
  — the two strings `setID` stored. `+0x50` is the `id` value (userId); `+0x10` appears to be
  the second `setID` string, which would put `deviceSN` in `ts` — the decompiler's local
  tracking here is ambiguous, so treat that pairing as **unverified** — to a Channel-A URL taken from
  the config (`+0x88`; `cleanPack/binding` is the likely endpoint, the only `*binding` string)
  and expects JSON with integer `code`: `0` = success, `0x66` (102) = re-register first.
  Retries each tick up to 61 times, then "send preBind msg timeOut..".
* **On success:** "send preBind msg success.." / "directly bind success..", sets
  `DAT_004c5ce4=1` (never read anywhere) and posts **EID `0x460` (1120)**, which is the
  "bind success" event other processes (UI/voice) react to — plausibly what ends the
  robot's "pairing mode" indication.
So `setID` does **not** itself POST anything: it *arms* the connection (heartbeat
`HeartOnlineLd+9` → read `ip_port.json` → dial Channel B) and the `BindUser` thread waits
for the link to come up before sending the bind. If the robot never reaches Channel B/A,
the cause is upstream of the bind POST.

### Re-home recipe (keep the vacuum alive on your own cloud)
1. Put the robot in pairing mode (soft-AP `LDRobot`, it listens on 192.168.78.1) and
   join that AP from your machine.
2. `getSn` to identify it (plain `getID` is not a command — see §C); `getWifi` to list networks.
3. **`setUrl`** → point it at your replacement server:
   `{"cmd":"setUrl","url":"http://192.168.1.x:8080/"}` (and/or the `ip`/`port` form to
   set the push gateway). This just writes `/data/bin/Run/Config/url` /
   `ip_port.json`.
4. **`setSta`** → `{"cmd":"setSta","staName":"<your-wifi>","staPwd":"<password>"}`, then
   **`applyCfg`** → `{"cmd":"applyCfg"}` to actually move it onto your LAN (`setSta` alone
   only stores the creds).
5. Stand up a server answering the channel-A endpoints your flow needs:
   `register` → return `data.session` (≥16 B; `session[:16]` is your AES key) **and**
   `data.cookies` (the sid, echoed back as `Cookie: cookies=…`); `getSockAddr` →
   `data.addr_list[]={ip,port}` of your gateway; plus `sync`/`response`/`uploadEvents`
   (accept the varied body templates in §A, `data`=plaintext JSON). On channel B: reply
   to the `10001` handshake, **answer every `21006` Ping** (keepalive — or the robot
   reconnects), and when you push a command, encrypt its `data` as
   `base64(AES-128-ECB(json, session[:16]))`. `eid_catalog.tsv` is the command map.

**Shortcut with shell access:** `setUrl`/`setSta` only write plaintext files
(`/data/bin/Run/Config/url`, `ip_port.json`, `/data/cfg/wpa_supplicant.conf`,
`/data/cfg/wifi_mode`). With root (UART/adbd/dropbear per `REPORT.md` §5) you can set
those directly and skip the UDP channel entirely.

---

## Answering the specific questions
* **REST API?** Yes for channel A (HTTP verbs + form/JSON). Channels B and C are *not*
  REST — B is a persistent framed-TCP JSON bus, C is UDP JSON datagrams.
* **HTTP or HTTPS?** Control API over **HTTPS** (`mobile.proscenic.cn`), but with **no
  cert validation**; **firmware/large blobs over plain HTTP** (OSS host).
* **Endpoints?** See the channel-A table (`cleanPack/*` + the version-check GET).
* **Data both sides expect?** Requests: form-urlencoded (A) or JSON (B/C).
  Responses: JSON, `{"code","message","data"}` (A) / `{"infoType","data"}` (B) /
  `{"cmd","result",…}` (C).
* **How to send local commands / re-home?** Channel C — UDP `{"cmd":…}` JSON to the
  robot in soft-AP mode (see recipe), or write the config files directly with root.
* **What is `setSta`?** It **stores** the Wi-Fi creds the robot will use in station mode
  (`{"cmd":"setSta","staName":…,"staPwd":…}`); the actual switch — `cleanpack_mode -m sta`,
  drop AP, join — happens on **`applyCfg`**. (Earlier revisions attributed the join to
  `setSta` itself; that was wrong.)

---

## Why the robot won't call your server after `setUrl`+`setSta` (rehome debug)

Traced end-to-end in `network_proxy` (the binary that is BOTH the local UDP command
dispatcher and the cloud client):

* **`setUrl` handler** = `FUN_00466af0`. It `WriteStrToFile("/data/bin/Run/Config/url", …)`
  **and** calls `FUN_004650c8`, which re-reads config and assigns the new URL into the live
  cloud-manager singleton (`*(singleton+0x128)`). So the URL is updated **both on disk and
  in memory** — good. But `FUN_004650c8` does **nothing else**: no session clear, no
  reconnect, no re-register.
* **The register session is in-memory and sticky.** `get_register.cpp` keeps `mSession` /
  `mCookies` as process members (no session file). The device registers (`cleanPack/register`
  → gets `session`+`cookies`) only when it has **no session** (`"no session!!!"`, on boot /
  first uplink) or when a Channel-A reply says the sid expired (`code 102/212`,
  `"COOKIES OUTTIME !!!"` / `"try register again!"`). After that it reuses the cached
  session and its cached gateway (`getSockAddr` result) — it does **not** re-register just
  because the URL changed.
* **`setSta`** (`cleanpack_mode -m sta`) only reconfigures `wpa_supplicant` / switches
  `wifi_mode`; it does **not** restart `network_proxy` or clear the session.
* There is **no local "reboot"/"reconnect" UDP command**. The only reboot path is the
  **cloud** command **`EID_C_REQUEST_REBOOT` (2023)** / `EID_C_REBOOT` (2024) over Channel B
  (`"App request reboot"`).

**Consequence:** if `network_proxy` already holds a session (robot was previously online, or
never restarted since boot), `setUrl`+`setSta` change the URL but the robot keeps using the
**old** session/gateway and never registers with your server. It only works from a genuinely
**session-free** state (fresh boot with no uplink yet, e.g. soft-AP pairing right after a
power-cycle) where the first uplink triggers `register` against the *new* URL.

**Fix for the rehome toolkit — force a re-registration after `setUrl`:**
1. **Simplest:** power-cycle / reboot the robot *after* `setUrl` (and `setSta`). On boot it
   has no session → first uplink → `register` to the new URL → connects to your server.
2. **With root** (UART/adbd/dropbear per REPORT.md §5): `killall network_proxy` (it respawns
   session-free and re-reads the URL) — a software-only trigger, no power cycle.
3. **What the app does:** it reboots the robot via the Channel-B `EID_C_REQUEST_REBOOT`
   command while the (still-alive) old cloud connection exists — i.e. the app's extra step is
   a **reboot/re-register trigger**, which the local `setUrl`+`setSta`-only flow omits.
   Equivalent local effect = option 1 or 2.

Order also matters: set the URL **before** the first uplink. Recommended local sequence:
`setUrl` → `setSta` → `applyCfg` → `setID` → reboot only if you need to *force
re-registration against a new URL* (the session-stickiness issue above). **Update
2026-10-04 (late):** for the **Channel-B address** (`setUrl ip/port`) a reboot is **not**
needed — the heartbeat applies it live (see the boxed steps in the Rehoming section); the
reboot advice in this section is about the *registration session*, not the address file.
And **never send `bindOk`** (`PAIRING_LOG_ANALYSIS.md` §5).

---

# Rehoming — pointing the robot at your server

> ## ‼ THE NECESSARY STEPS
> 1. **Overwrite `ip_port.json`** — the robot chooses its push-gateway (Channel B) from
>    **`/data/bin/Run/Config/ip_port.json`**, read first and sticky across reboots. Write it
>    with `setUrl` in its **ip+port form**: `{"cmd":"setUrl","ip":"<your host>","port":<port>}`.
>    `setUrl {"url":…}` alone changes only the cloud base URL — **never** use it for the B
>    target. **No reboot is needed:** `setUrl ip/port` updates the live connection object
>    (`FUN_00454458` SetIpPort) and the 1 s heartbeat loop applies the new address
>    (run-log line `mIpAndPort : [ip:port]`). *(An earlier revision said to power-cycle
>    here — that was wrong; the reboot advice belongs to the session/URL machinery, not the
>    address file.)*
> 2. **Arm it with `setID`** (`{"id":<userId>,"deviceSN":"<SN>"}`) — this is what makes the
>    robot resolve the address and start dialing your Channel-B server. Then the full chain
>    must succeed: B handshake `10001` + `21006` pongs → Channel-A preBind answered
>    `code:0` → EID `0x460` ends pairing. See `PAIRING_LOG_ANALYSIS.md` §4/§8.
> 3. **Never send `bindOk`** — it kills the local channel and starts the fork storm
>    (see its table row in §C).

_This section corrects an earlier draft that wrongly said the robot speaks the app's
20-byte imsocket protocol. **It does not.** The robot's control link is **Channel B**
(§B above). The 20-byte imsocket protocol is **Channel A** — the phone↔cloud link
(`WIRE_PROTOCOL.md`). The cloud bridges A↔B. A headless rehome implements **Channel B
only**; you talk to the robot directly._

## Which channel your server must speak

| | Channel A (app↔cloud) | Channel B (robot↔cloud) — **rehome target** |
|---|---|---|
| Endpoint | `bl-im-<region>.robotbona.com:20008` (hardcoded in app, CN default `bl-im.robotbona.com`) | ip:port from `/data/bin/Run/Config/ip_port.json` (Proscenic push gateway, `proscenic.cn`) |
| Framing | 20-byte LE header + JSON (`WIRE_PROTOCOL.md`) | `<JSON>` + 3-byte delimiter `#\t#` (`23 09 23`), §B |
| Login | cmd 16, `{appId,clientType,token,userId,uuid,userType}` | `{"infoType":10001,"connectionType":1,"data":{"token":"","sn":"<SN>"}}` |
| Commands | TRANSIT (250) wrapping `ImMessage` | `{"encrypt":0|1,"data":<MESSAGE>}` — `<MESSAGE>` = `{"infoType":N,"data":{…},"dInfo":{…}}` (double nesting, `CHANNEL_B_INBOUND.md`) |
| Who connects here | the phone app | **the robot** |

The robot never opens a Channel-A/imsocket connection (the firmware contains no
`robotbona`, `bl-im`, `20008`, or imsocket login keys). You only need Channel A if you
also want to run the *real phone app* against your server, which additionally requires
redirecting the app's hardcoded `bl-im` host — out of scope for a headless rehome.

## How the robot chooses its Channel-B target

`get_tcp_addr.cpp` (`FUN_00451cd0`), in priority order (re-verified by disassembly 2026-10-05):
1. `/data/bin/Run/Config/ip_port.json` = `{"ip":"<addr>","port":<int>}` (keys literally
   `ip`/`port`; writer is the `setUrl ip/port` handler `FUN_00466af0`) — used directly if
   present and valid. **This path makes no HTTP request at all**, so when it works there is
   no Channel-A traffic before the Channel-B dial.
2. Otherwise **`GET <base>cleanPack/getSockAddr?version=1&sn=<sn>&companyId=<n>`** (curl,
   5 s timeout) → parse `{"code":0,"data":{"addr_list":[{"ip":"…","port":N}]}}` (key bytes
   confirmed in the binary: `code`, `data`, `addr_list`, element `ip`/`port`). On failure:
   0.2 s sleep, then the outer 1 s retry — so a rejected local file makes the robot poll
   getSockAddr roughly every 1.2 s.

`FUN_00454998` is the blocking resolver/loop around it; it hands the list to the TCP client
and returns only once ≥1 address exists. It runs (a) **unconditionally at heartbeat-thread
start** (i.e. at every boot — the thread is started by `main` → manager `Start` →
`HeartOnlineLd::Start`, even in AP mode) and (b) whenever `setID` arms the connect flag
(`HeartOnlineLd+9`). Immediately after it returns, the heartbeat calls
`TcpClientPort::Connect` (`FUN_0046a588`) + sends the `10001` handshake — **the TCP SYN is
the first packet the rehome can produce**, and it follows within ~1 s of a valid
`ip_port.json` existing.

**Boot caveat (URL composition):** the base for all `cleanPack/*` endpoints is
`/data/bin/Run/Config/url`, but the endpoint strings are only *rebuilt* by `FUN_004650c8`
when a `setUrl` command runs (`<base> + cleanPack/register|binding|getSockAddr|…`). Before
the first `setUrl(url)` of a process lifetime they are relative (`cleanPack/…`), curl
rejects them locally without emitting a packet, and getSockAddr polling is silent. Default
base (file unreadable) is the vendor `https://mobile.proscenic.cn/`.

`FUN_00455090` is the heartbeat/reconnect loop that maintains the Channel-B TCP connection
(handshake `10001`, pings `21006`, close+redial on `isOpen==0`).

## `setUrl` has TWO modes (verified this session, `FUN_00466af0`, `wifi_config.cpp`)

- `{"url":"..."}` → writes `/data/bin/Run/Config/url` and reloads the HTTP base
  (`FUN_004650c8`). **Changes only the registration endpoint.**
- `{"ip":"...","port":<int>}` → writes `/data/bin/Run/Config/ip_port.json` **directly**
  **and** updates the live connection object (`FUN_00454458` SetIpPort: stores the ip/port
  fields, marks the address valid). The 1 s heartbeat loop then applies it
  (`FUN_00455090`: builds the address list, hands it to the TCP client, logs
  `mIpAndPort : [ip:port]` in the run log). **No reboot / restart is required** — verified
  live 2026-10-04; an earlier revision of this file claimed otherwise, which was wrong. The
  dial itself is the heartbeat's normal job (it reconnects on its ~15 s cadence / whenever
  the link is down); `setID` additionally arms the resolve-and-connect path
  (`HeartOnlineLd+9`) that drives the bind.

## Why `setUrl`(url)+`setSta` alone did NOT make the robot call your server

- `ip_port.json` is read **first** and, after normal pairing, still holds Proscenic's
  gateway; changing only the `url` file leaves the live connection pointed at
  Proscenic, and a reboot re-reads the same `ip_port.json`.
- Clearing `ip_port.json` forces HTTP registration, whose response must carry a valid
  `data.session`+`data.cookies` (§B); a naive server that returns the wrong shape
  yields `no session!!!` / `decrypt data failed` and the robot never connects.

## Minimal rehome procedure

1. `setUrl {"ip":"<your host>","port":<port>}` → writes `ip_port.json` (or run a
   register endpoint per §B that returns your ip:port + `session`/`cookies`).
2. Run a **Channel-B** server at that ip:port (see `rehome_server.py` and §B):
   - accept the TCP connection; read `#\t#`-framed frames;
   - on the `{"infoType":10001,…}` handshake, record the `sn`;
   - if the robot registers over HTTP, mint `data.session` (≥16 bytes → your AES key)
     and any `data.cookies`; `sig`/RSA can be a no-op (the robot only sends `sig`);
   - **pong every `{"infoType":21006}` immediately** with an enveloped `#\t#`-terminated
     frame — `{"encrypt":0,"data":{"infoType":21006,"data":{}}}#\t#` — or the robot loops
     connect→drop→reconnect (a flat pong only refreshes the timer; the nested one also
     enables status pushes — `CHANNEL_B_INBOUND.md`);
   - send commands as `{"encrypt":0,"data":{"infoType":N,"data":{…},"dInfo":{"ts":"…","userId":"…"}}}#\t#`
     — **double nesting**: the robot dispatches the contents of the outer `data`, and
     *that* object must carry the integer `infoType` (`CHANNEL_B_INBOUND.md`; use
     `encrypt:0` to skip AES entirely; `dInfo.ts`/`dInfo.userId` must be strings for any
     command that should reply).
3. Send **`setID`** (`{"id":…,"deviceSN":"<SN>"}`) to arm the bind and watch your B server
   for the `10001` handshake. Pairing ends only after the full chain succeeds
   (B handshake + `21006` pongs → Channel-A preBind `code:0` → EID `0x460`). **Do not send
   `bindOk`.** No reboot is needed for the address file (see the boxed steps above).

Cross-references: full Channel-B spec → §B above; command vocabulary → `COMMANDS.md`;
app-side Channel-A imsocket protocol → `WIRE_PROTOCOL.md`; extraction of the app →
`UNPACKING.md`.
