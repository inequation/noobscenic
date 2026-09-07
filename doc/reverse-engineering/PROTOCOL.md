# Proscenic M7 Pro — Device ↔ Cloud & Local Control Protocol

Reconstructed from `network_proxy` (`src/interface/product/ld/*`, `src/local/*`,
`src/component/https_base`), `libcpc.so`, the boot/provisioning scripts, and live
probing of the vendor cloud. **[proven]** = executed/verified, **[static]** =
from decompiled code, **[live]** = confirmed against the running server.

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
| `cleanPack/binding` / `cleanPack/unbinding` | POST | bind/unbind device to a user account | sn, username/token |
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

**Firmware download URL** is a *signed* OSS request **[static]**:
```
<oss>/robotDrivers/<ms-timestamp>?itemId=5&clientId=11&fileTypeId=<n>
   &fileName=<name>&from=mpc_clean_firmware&time=<t>&_taskid=<tid>&_sign=<sig>
```
The downloaded file is the RSA-signed container from `REPORT.md` §1.

### Bootstrap order, binding, and two open gaps a server author must close **[static]**
* **Order:** `register` (get `session`+`cookies`) → `getSockAddr` (get gateway
  `ip:port`, sent with the cookie) → open the Channel-B TCP socket → `10001`
  handshake → steady state. `getSockAddr` runs through the same cookie-setting curl
  path as the reporters, so the cookie exists by then.
* **⚠ Gap 1 — `getSockAddr` request body:** the exact POST fields (`sn`? `ts`? `data`?)
  were not cheaply extractable. A server should **not require** unknown fields; verify
  the body from one capture and treat the request as cookie-authenticated.
* **⚠ Gap 2 — binding state machine (`bind_user.cpp`):** there is a `BindUser` flow
  the endpoint one-liners understate: `StartBind`, `need to update bind, BDS_WAIT_CONN
  -> BDS_SEND_BIND..`, `send preBind msg success..`, `directly bind success..`,
  `check bind status timeOut..`, `Bind Timeout`, `Recv unexpect SetBindSuccess funccall`;
  request templates `sn=%s&ts=%s&userId=%s` and `devType=3&sn=%s&ts=%s&qid=%s`.
  A freshly re-homed device may sit in a preBind/retry state. **It is not established
  whether map upload / command execution is gated on bind success** — determine this
  from a capture or by testing the replacement server; if gated, the server must drive
  the device to `SetBindSuccess`.

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
* **Message object:** `{"infoType":<int>, "data":{…}}`. Server→device replies and
  device push-acks also use `"message":"ok"|"fail"`, `"reason":"<str>"` (on errors),
  and `"packId":<int>` for chunked transfers.

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
* **cloud → device** command: the message object **MUST carry an integer `encrypt`
  field alongside `data`** — this is a hard gate. Inbound callback `FUN_00457f08`
  (`interface_obj.cpp`, the connection onMessage) does, in order (all verified):
  1. `msg["encrypt"].isInt()` — **if not an integer, the frame is dropped** (member
     string `"encrypt"` @ `0x490950`).
  2. `msg.isMember("data")` — **if `data` is absent, dropped** (`"data"` @ `0x488658`).
  3. `e = msg["encrypt"].asInt()`. **`e != 1` ⇒ `data` is used as a plaintext JSON
     object** directly; **`e == 1` ⇒ `data` is a string** =
     `base64( AES-128-ECB( json, key = SESSION[0:16] ) )` and is decrypted
     (`FUN_00457da0` → `FUN_004789a8`: `EVP_aes_128_ecb`, **padding disabled**
     `EVP_CIPHER_CTX_set_padding(ctx,0)`, key size `0x10` = 16 bytes; on failure:
     `"decrypt data failed. try register again!"`).

  So there are **two valid server→device command forms**:
  ```
  {"infoType":<N>,"encrypt":0,"data":{ …plaintext JSON object… }}      # no crypto needed
  {"infoType":<N>,"encrypt":1,"data":"<base64(AES-128-ECB(json,SESSION[:16]))>"}
  ```
  **`encrypt:0` lets a replacement server skip AES entirely** — the simplest correct
  path. A command with no `encrypt` field is silently dropped.
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

**Pong contract (recovered statically):** the pong handler `FUN_00457b30` calls
`RecvPong` (`FUN_00454880`), which **marks the device online on receipt alone**
(sets the online flag and stores `GetCurrentTickSec`). It then optionally reads a
boolean member **`isExistConnect`** (`@0x490910`) and stores it (app-online reflection);
if absent it just logs `"on new ping pong msg..."`. So a safe pong is:
```
{"infoType":21006,"data":{}}            # or add "isExistConnect":true
```
(There **is** an infoType-keyed inbound dispatch table — `response_handle`, an
`std::map<int,std::function>` that logs `"Error : Unknown infoType %d"`, with `21006`
among the registered keys; the exact slot that routes a frame to the pong handler
`FUN_00457b30` was not traced, so confirm the pong `infoType` against one capture.
`21006` is the near-certain pong type.)

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
| 21020 | both | chunked pack transfer (`packId`; error `reason:"invalid json …"`) |
| 70001 | app-side only | crypto-token message seen in the RE-repo capture; the string `70001` is **absent from every robot binary**, so it is **not part of the robot's protocol** and irrelevant to a replacement server (the robot's AES key is the register `session`, not this). |

Note: `infoType` on the wire and the internal `EID_*` bus IDs are **different number
spaces** — `EID_*` (0–7999, see `eid_catalog.tsv`) are the in-process ZeroMQ event
bus IDs (`ZmqManager::PostEvent(EventId,…)`); `infoType` are the cloud-facing message
type codes. The device maps between them in `response_handle.cpp`.

---

## (C) Local config channel — UDP JSON `{"cmd":…}`  ← this is how you re-home it

Used by the app on the **local network** (most importantly while the robot is in its
`LDRobot` soft-AP: `apDemo` brings up AP at **192.168.78.1**, DHCP 192.168.78.50-150).
Handled by `network_proxy` (`local/local_debugger.cpp`, `interface/.../wifi_config.cpp`,
`udp_server_interface.cpp`). **[static]**

* **Transport:** **UDP datagrams** carrying a JSON object; the device replies with a
  JSON datagram. The device’s LAN control server (`OpenUdpRemoteCtrl`, `cp_function.cpp`)
  binds a **UDP port chosen as `rand()%1000 + 9000` (9000–9999)**; the app discovers
  the device/port via a broadcast `getID` exchange (`getID` → `{"result":"ok","type":
  "ipfromapp"}`). **[static]** *(The command set and JSON schemas below are fully
  recovered; the exact discovery port is dynamic — capture one real pairing session,
  or just write the config files directly if you have shell access, see “Re-home”.)*
* **Request shape:** `{"cmd":"<name>", …fields…}` → **Response:**
  `{"cmd":"<name>","result":"ok"|"fail","code":<int>, …}`; unknown → `{"result":"invalue cmd"}`.

### Command reference **[static]**
| `cmd` | Request fields | Response | Effect |
|---|---|---|---|
| `getID` | – | `{"result":"ok","type":"ipfromapp"}` | discovery / identify |
| `getSn` | – | `{"result":"ok","sn":"<serial>"}` | read serial (from `/data/bin/sys_data/custom.sn`) |
| `getWifi` | – | `{"result":"ok","wifi_list":[ … ]}` | scan nearby APs |
| `checkPwd` | – | `{"result":"ok","code":<connState>}` | query Wi-Fi/join status |
| `getCfg` | – | `{"result":"ok", …, "staName":"…","staPwd":"…","staIp":"…","staMac":"…"}` | current STA/config |
| `applyCfg` | config object | `{"result":"ok","code":1}` / `fail,-1` | apply config |
| **`setSta`** | **`{"ssid":"<name>","staPwd":"<8-64 char pass>"}`** | `{"result":"ok","code":2}` / `fail,-1` | **join a Wi-Fi network as client** (see below) |
| `setAp` | ap params | `{"result":"ok"}` / `fail` | switch back to soft-AP |
| **`setUrl`** | **`{"url":"http://you/"}`** *or* **`{"ip":"1.2.3.4","port":<n>}`** | `{"result":"ok"}` | **set cloud base URL / push-gateway** |
| `resetWifi` | – | `{"result":"ok"/"fail"}` | clear Wi-Fi config |
| `{"req":"getLog"/"rmLog"}` | offset | log package (base64) | pull/remove logs |

### What `setSta` is
**“Set Station mode.”** Wi-Fi has two roles: **AP** (the robot *is* the access point,
`LDRobot`, used for pairing) and **STA/station** (the robot is a *client* that joins
your router). `setSta` hands the robot the SSID + password of the network it should
join. Handler `FUN_00464cb8` validates the password length (**8–64 chars**) and runs:
```
sh /data/bin/cleanpack_mode -m sta -s "<ssid>" -p "<password>"
```
which writes `wpa_supplicant.conf`, switches `wifi_mode`→`sta`, tears down the AP and
associates to your router. (Field names taken from the firmware’s JSON templates —
`ssid` + `staPwd`; confirm against one real app capture if a byte-exact client is
needed.)

### Re-home recipe (keep the vacuum alive on your own cloud)
1. Put the robot in pairing mode (soft-AP `LDRobot`, it listens on 192.168.78.1) and
   join that AP from your machine.
2. `getID`/`getSn` to discover it; `getWifi` to list networks.
3. **`setUrl`** → point it at your replacement server:
   `{"cmd":"setUrl","url":"http://192.168.1.x:8080/"}` (and/or the `ip`/`port` form to
   set the push gateway). This just writes `/data/bin/Run/Config/url` /
   `ip_port.json`.
4. **`setSta`** → `{"cmd":"setSta","ssid":"<your-wifi>","staPwd":"<password>"}` to move
   it onto your LAN.
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
* **What is `setSta`?** The command that puts the robot in Wi-Fi **station** mode and
  joins it to a given SSID/password (shells out to `cleanpack_mode -m sta`).
